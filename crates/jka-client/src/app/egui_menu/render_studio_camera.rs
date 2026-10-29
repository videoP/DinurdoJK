use super::App;

pub(super) fn studio_lens_editor(ui:&mut egui::Ui,id:impl std::hash::Hash+std::fmt::Debug,
    fov:&mut f32,lens:&mut crate::renderer::StudioLens,aspect:f32,target_bound:bool)->bool {
    use super::render_studio_ui as sui;
    let before=(*fov,*lens);
    sui::prop_slider(ui,"FOV",egui::Slider::new(fov,10.0..=140.0).suffix("°"));
    sui::prop_value(ui,"35 mm equivalent",&format!("{:.1} mm",crate::renderer::StudioLens::focal_length_35mm(*fov,aspect)));
    studio_aperture_editor(ui,lens,target_bound);
    let _=id;
    ui.label(egui::RichText::new("35 mm matches horizontal framing at this preview aspect.").small());
    before!=(*fov,*lens)
}
pub(super) fn studio_aperture_editor(ui:&mut egui::Ui,lens:&mut crate::renderer::StudioLens,target_bound:bool)->bool {
    use super::render_studio_ui as sui;
    let before=*lens;
    let mut infinite=lens.aperture.is_none();
    if ui.checkbox(&mut infinite,"f/∞ · everything in focus").changed(){lens.aperture=if infinite{None}else{Some(2.8)};}
    if let Some(n)=lens.aperture.as_mut(){
        sui::prop_drag(ui,"Aperture",egui::DragValue::new(&mut *n).range(0.7..=128.0).prefix("f/").speed(0.1));
        ui.add_enabled_ui(!target_bound,|ui|{sui::prop_drag(ui,"Focus distance",egui::DragValue::new(&mut lens.focus_distance).range(1.0..=65536.0).speed(1.0).suffix(" units"));});
        if target_bound {sui::hint(ui,"Focus follows the camera target in 3D space.");}
        ui.horizontal_wrapped(|ui|{for stop in [1.0,1.4,2.0,2.8,4.0,5.6,8.0,11.0,16.0,22.0]{
            if ui.selectable_label((*n-stop).abs()<0.01,format!("f/{stop}")).clicked(){*n=stop;}
        }});
    }
    before!=*lens
}
use super::render_studio_ui::palette as theme;
use super::render_studio_ui::{self as sui, ButtonKind, Icon};
use crate::app::render_studio_camera::{StudioAttachmentPart,StudioCameraAttachment,StudioCameraOrbit};

fn bone_label(name:&str)->String {
    match name {
        "@origin"=>"Actor origin".into(),"@hilt"=>"Hilt / hand grip".into(),
        _=>name.strip_prefix("@tip").map_or_else(||name.to_owned(),|i|format!("Blade {i} tip")),
    }
}

impl App {
    pub(super) fn egui_studio_camera_rig(&mut self,ui:&mut egui::Ui) {
        sui::section(ui,"studio_camera_rig","Camera rig",
            Some("Follow / aim attachments are shared across the whole camera track. Distance, orbit angles and FOV belong to individual camera keys and animate between them; editing a selected key leaves the others unchanged."),
            true,|ui| {
                let time=self.render_studio.timeline_time();
                let visible=self.studio_shot_camera();
                let preview_eye=self.asset_preview_camera().position.to_array();
                let edit_index=self.render_studio.timeline.selected_camera.filter(|&i|i<self.render_studio.timeline.camera.len());
                let edit_key=edit_index.map(|i|self.render_studio.timeline.camera[i]).unwrap_or_else(|| {
                    let mut key=self.render_studio.timeline.camera_at(time);key.time=time;key
                });
                let edit_frame=self.studio_orbit_edit_frame(edit_key.time);
                let mut rig=self.render_studio.timeline.camera_rig.clone();
                let previous=rig.clone();
                let lane=self.render_studio.timeline.selected_clip
                    .or_else(||self.render_studio.timeline.selected_motion.map(|(clip,_)|clip))
                    .and_then(|i|self.render_studio.timeline.clips.get(i)).map_or(0,|c|c.lane);
                let mut match_view=None;
                let mut detach=false;
                let mut orbit_edit=None;
                let mut convert_space=None;
                sui::group_label(ui,"Presets");
                match sui::button_row(ui,&[(None,"Track head",ButtonKind::Normal,true),
                    (None,"Orbit target",ButtonKind::Normal,true),(None,"Bolt to saber",ButtonKind::Normal,true)]) {
                    Some(0)=>{rig.target=StudioCameraAttachment {enabled:true,lane,half_life:0.12,..Default::default()};}
                    Some(1)=>{
                        let anchor=if rig.target.enabled {rig.target.clone()}
                            else if rig.position.enabled {let mut anchor=rig.position.clone();anchor.offset=[0.;3];anchor}
                            else {StudioCameraAttachment {lane,half_life:0.12,..Default::default()}};
                        if anchor.part!=StudioAttachmentPart::Player {self.set_studio_actor_sabers_enabled(anchor.lane,true);}
                        rig.target=anchor.clone();rig.target.enabled=true;
                        rig.position=StudioCameraAttachment {enabled:true,lane:anchor.lane,part:anchor.part,bone:anchor.bone,
                            local_offset:rig.position.local_offset,half_life:rig.position.half_life,..Default::default()};
                        match_view=Some(visible.position.to_array());
                    }
                    Some(2)=>{
                        self.set_studio_actor_sabers_enabled(lane,true);
                        rig.position=StudioCameraAttachment {enabled:true,lane,part:StudioAttachmentPart::Saber1,
                            bone:"@hilt".into(),local_offset:true,half_life:0.08,..Default::default()};
                        rig.target=StudioCameraAttachment {enabled:true,lane,half_life:0.12,..Default::default()};
                        match_view=Some(visible.position.to_array());
                    }
                    _=>{}
                }
                if sui::button_row(ui,&[(None,"Detach rig (keep current shot)",ButtonKind::Ghost,rig.active())]).is_some() {detach=true;}
                if self.render_studio.spatial.scene_camera.is_some() {
                    ui.horizontal(|ui| {
                        sui::hint(ui,"Scene view is an observer.");
                        if sui::button(ui,Some(Icon::Camera),"Show shot",ButtonKind::Normal,true)
                            .on_hover_text("Look through the camera controlled by this rig").clicked() {
                            self.render_studio.spatial.scene_camera=None;self.render_studio.spatial.drag=None;self.profile_preview_key=None;
                        }
                    });
                }
                for i in 0..2 {
                    ui.push_id(("camera_attachment",i),|ui| {
                        ui.add_space(6.0);
                        let binding=if i==0 {&mut rig.position}else{&mut rig.target};
                        let before=binding.clone();
                        sui::group_label(ui,if i==0 {"Camera position"}else{"Aim target"});
                        sui::prop_toggle(ui,if i==0 {"Follow"}else{"Track"},&mut binding.enabled)
                            .on_hover_text(if i==0 {"Follow a character bone with the camera position"}else{"Aim the camera at a character bone"});
                        if !binding.enabled {
                            sui::hint(ui,if i==0 {"Position comes from camera keys. Use Orbit target to follow and orbit a bone."}
                                else {"Direction comes from camera-key yaw and pitch."});
                            return;
                        }
                        let layers:Vec<(usize,String)>=(0..self.render_studio.timeline.lanes).map(|l|(l,format!("Character {}",l+1))).collect();
                        let layers:Vec<(usize,&str)>=layers.iter().map(|(l,s)|(*l,s.as_str())).collect();
                        sui::prop_combo(ui,"Character","layer",&mut binding.lane,&layers);
                        let old_part=binding.part;
                        let parts:Vec<(StudioAttachmentPart,&str)>=[StudioAttachmentPart::Player,StudioAttachmentPart::Saber1,StudioAttachmentPart::Saber2]
                            .into_iter().map(|p|(p,p.label())).collect();
                        sui::prop_combo(ui,"Part","part",&mut binding.part,&parts);
                        if old_part!=binding.part {
                            binding.bone=if binding.part==StudioAttachmentPart::Player {"*head_top"}else{"@hilt"}.into();
                            self.render_studio.camera_bone_search[i].clear();
                        }
                        if binding.part!=StudioAttachmentPart::Player {
                            let mut enabled=self.render_studio.actor_with_sabers(binding.lane);
                            if sui::prop_toggle(ui,"Sabers",&mut enabled).on_hover_text("Show this character track's sabers").changed() {
                                self.set_studio_actor_sabers_enabled(binding.lane,enabled);
                            }
                        }
                        let info=self.studio_attachment_info(binding.lane);
                        let names=self.ensure_asset_preview_player_presenter().and_then(|_|self.asset_preview_player_presenter.as_mut()
                            .ok_or_else(||"Player presenter unavailable".to_owned())?
                            .profile_attachment_names(&info,binding.part.saber()));
                        sui::prop_row(ui,"Bone",|ui| {
                            sui::combo_box("bone").width(ui.available_width())
                                .height(300.).wrap_mode(egui::TextWrapMode::Truncate)
                                .selected_text(bone_label(&binding.bone)).show_ui(ui,|ui| {
                                    ui.add(egui::TextEdit::singleline(&mut self.render_studio.camera_bone_search[i])
                                        .hint_text("Search bones / tags").desired_width(230.));
                                    match names.as_ref() {
                                        Ok(names)=>{
                                            let needle=self.render_studio.camera_bone_search[i].trim().to_ascii_lowercase();
                                            let filtered:Vec<_>=names.iter().filter(|s|needle.is_empty()
                                                ||s.to_ascii_lowercase().contains(&needle)||bone_label(s).to_ascii_lowercase().contains(&needle)).collect();
                                            ui.small(format!("{} attachments",filtered.len()));
                                            egui::ScrollArea::vertical().max_height(220.).show_rows(ui,22.,filtered.len(),|ui,rows| {
                                                for row in rows {
                                                    let name=filtered[row];
                                                    if ui.add_sized([230.,22.],egui::Button::new(bone_label(name))
                                                        .selected(binding.bone==*name).truncate()).clicked() {
                                                        binding.bone=name.clone();ui.close();
                                                    }
                                                }
                                            });
                                        }
                                        Err(error)=>{ui.small(error);}
                                    }
                                });
                        });
                        sui::prop_row(ui,"",|ui| {
                            ui.spacing_mut().item_spacing.x=3.0;
                            let quick: &[(&str,&str)]=if binding.part==StudioAttachmentPart::Player {
                                &[("Origin","@origin"),("Head","*head_top"),("R hand","*r_hand"),("L hand","*l_hand")]
                            } else {&[("Hilt","@hilt"),("Blade base","*blade1"),("Tip","@tip1")]};
                            for (label,name) in quick {
                                let active=binding.bone==*name;
                                if sui::button(ui,None,label,if active {ButtonKind::Primary} else {ButtonKind::Ghost},true).clicked() {
                                    binding.bone=(*name).into();
                                }
                            }
                        });
                        let runtime_binding=if i==0 {&previous.position}else{&previous.target};
                        let frame=if i==0 {edit_frame}else{self.render_studio.camera_rig_runtime.frames[i]};
                        let frame=frame.filter(|_|runtime_binding.enabled
                            && runtime_binding.lane==binding.lane && runtime_binding.part==binding.part && runtime_binding.bone==binding.bone);
                        let bone_space_ok=frame.is_none_or(|frame|
                            glam::Mat3::from_cols_array_2d(&frame.axis).determinant().abs()>1e-6);
                        let mut local=binding.local_offset;
                        sui::prop_row(ui,if i==0 {"Orbit space"}else{"Offset space"},|ui| {
                            ui.add_enabled_ui(bone_space_ok || local,|ui| sui::segmented(ui,&mut local,&[(false,"World"),(true,"Bone")])).response
                        }).on_hover_text("World: fixed directions, Y up. Bone: rotates with the attachment, local Z up. Switching keeps the current position. If bone axes collapse during smoothing, reduce smoothing or use World.");
                        binding.local_offset=local;
                        // Changing spaces preserves the currently displayed attachment point.
                        if binding.local_offset!=before.local_offset {
                            if i==0 {convert_space=Some(binding.local_offset);}
                            if let Some(frame)=frame {
                                binding.offset=frame.offset(frame.point(before.offset,before.local_offset),binding.local_offset);
                            }
                        }
                        if i==0 {
                            sui::hint(ui,&edit_index.map_or_else(||format!("At {:.3}s · editing adds a camera key",edit_key.time),
                                |i|format!("Camera key {} · {:.3}s",i+1,edit_key.time)));
                            if let Some((frame,mut editor_binding,mut orbit))=frame.and_then(|frame| {
                                let point=previous.position_point(edit_key,frame);
                                let eye=std::array::from_fn(|a|point[a]+edit_key.position[a]-previous.reference_position[a]);
                                let mut editor_binding=binding.clone();editor_binding.offset=frame.offset(eye,binding.local_offset);
                                StudioCameraOrbit::from_binding(&editor_binding,frame,[0.;3]).map(|orbit|(frame,editor_binding,orbit))
                            }) {
                                let mut changed=false;
                                changed|=sui::prop_drag(ui,"Distance",egui::DragValue::new(&mut orbit.distance)
                                    .speed(1.).range(0.0..=50000.0).clamp_existing_to_range(false).max_decimals(2).suffix(" u"))
                                    .on_hover_text("Distance at this camera key. It interpolates to the next key's distance. FOV changes framing separately.").changed();
                                ui.add_enabled_ui(orbit.distance>0.001,|ui| {
                                    changed|=sui::prop_slider(ui,"Orbit yaw",egui::Slider::new(&mut orbit.yaw,-180.0..=180.0).suffix("°")).changed();
                                    changed|=sui::prop_slider(ui,"Orbit pitch",egui::Slider::new(&mut orbit.pitch,-90.0..=90.0).suffix("°")).changed();
                                });
                                if changed {orbit_edit=Some((edit_key.time,orbit));orbit.apply(&mut editor_binding,frame,[0.;3]);}
                                let offset_before=editor_binding.offset;
                                egui::CollapsingHeader::new(egui::RichText::new("Advanced XYZ offset (this key)").size(11.5).color(theme::TEXT_DIM))
                                    .id_salt("xyz_offset").show(ui,|ui| {
                                    sui::prop_vec3(ui,"Offset",&mut editor_binding.offset,0.25,["X","Y","Z"],true);
                                    if sui::button_row(ui,&[(None,"Zero offset",ButtonKind::Ghost,true)]).is_some() {editor_binding.offset=[0.;3];}
                                });
                                if editor_binding.offset!=offset_before {
                                    if let Some(orbit)=StudioCameraOrbit::from_binding(&editor_binding,frame,[0.;3]) {orbit_edit=Some((edit_key.time,orbit));}
                                }
                            } else {sui::hint(ui,"Orbit controls appear once the bone is ready. For unstable Bone axes, use World space or less smoothing.");}
                            if sui::button_row(ui,&[(Some(Icon::Camera),"Use preview position",ButtonKind::Normal,true)]).is_some() {
                                match_view=Some(preview_eye);
                            }
                            if !before.enabled || before.lane!=binding.lane || before.part!=binding.part || before.bone!=binding.bone {match_view=Some(visible.position.to_array());}
                        } else {
                            sui::prop_vec3(ui,"Aim offset",&mut binding.offset,0.25,["X","Y","Z"],true);
                            if sui::button_row(ui,&[(None,"Center on bone",ButtonKind::Ghost,true)]).is_some() {binding.offset=[0.;3];}
                        }
                        sui::prop_slider(ui,"Smoothing",egui::Slider::new(&mut binding.half_life,0.0..=2.0).suffix(" s"))
                            .on_hover_text("0 follows rigidly. Higher values soften movement with more lag: the time to remove half the error. Preview, scrubbing and exports match.");
                        sui::prop_row(ui,"",|ui| {
                            ui.spacing_mut().item_spacing.x=3.0;
                            for (label,value) in [("Rigid",0.),("Gentle",0.12),("Smooth",0.3)] {
                                let active=(binding.half_life-value).abs()<0.001;
                                if sui::button(ui,None,label,if active {ButtonKind::Primary} else {ButtonKind::Ghost},true).clicked() {binding.half_life=value;}
                            }
                        });
                        if let Some(message)=self.render_studio.camera_rig_runtime.messages[i].as_ref() {
                            sui::callout(ui,message,theme::WARNING);
                        }
                    });
                }
                if detach {rig.position.enabled=false;rig.target.enabled=false;}
                if let Some(local)=convert_space {self.convert_studio_orbit_space(local);}
                let source_off=previous.position.enabled && !rig.position.enabled;
                let target_off=previous.target.enabled && !rig.target.enabled;
                if source_off || target_off {
                    self.render_studio.timeline.begin_camera_edit();
                    let index=self.render_studio.timeline.selected_camera.unwrap();
                    let key=&mut self.render_studio.timeline.camera[index];
                    if source_off {key.position=visible.position.to_array();}
                    if target_off {key.yaw=visible.yaw.to_degrees();key.pitch=visible.pitch.to_degrees();}
                }
                if previous!=rig || match_view.is_some() || detach {
                    self.render_studio.timeline.camera_rig=rig;
                    if let Some(eye)=match_view.filter(|_|self.render_studio.timeline.camera_rig.position.enabled) {
                        self.render_studio.timeline.seek(time);
                        self.render_studio.camera_pending_match=Some(eye);
                    } else if !self.render_studio.timeline.camera_rig.position.enabled {self.render_studio.camera_pending_match=None;}
                    self.update_studio_camera_rig();
                    self.profile_preview_key=None;
                }
                if let Some((time,orbit))=orbit_edit {
                    if !self.set_studio_orbit_key(time,orbit) {self.render_studio.status="Orbit key could not be updated: wait for the follow bone to load or use World space".into();}
                }
            });
    }
}

pub(super) fn studio_target_editor(ui:&mut egui::Ui,target:&mut Option<[f32;3]>)->bool {
    let before=*target;
    if let Some(point)=target {
        sui::prop_vec3(ui,"Target (world)",point,1.0,["X","Y","Z"],true);
        if ui.button("Clear target override").clicked(){*target=None;}
    } else {sui::hint(ui,"In Scene view, drag the camera target to set aim and focus.");}
    before!=*target
}
