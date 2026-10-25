use super::{App,theme};
use crate::app::render_studio_camera::{StudioAttachmentPart,StudioCameraAttachment,StudioCameraOrbit};

fn bone_label(name:&str)->String {
    match name {
        "@origin"=>"Actor origin".into(),"@hilt"=>"Hilt / hand grip".into(),
        _=>name.strip_prefix("@tip").map_or_else(||name.to_owned(),|i|format!("Blade {i} tip")),
    }
}

impl App {
    pub(super) fn egui_studio_camera_rig(&mut self,ui:&mut egui::Ui) {
        egui::CollapsingHeader::new(egui::RichText::new("CAMERA RIG").color(theme::ACCENT).strong())
            .id_salt("studio_camera_rig").default_open(true).show(ui,|ui| {
                ui.small("Follow/aim setup is shared across the track. Distance and orbit angles belong to individual camera keys.");
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
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Track head").on_hover_text("Aim at the selected character's head with gentle damping").clicked() {
                        rig.target=StudioCameraAttachment {enabled:true,lane,half_life:0.12,..Default::default()};
                    }
                    if ui.button("Orbit target").on_hover_text("Follow and aim at the selected target bone, preserving the shot position. Then adjust Distance, Orbit yaw and Orbit pitch.").clicked() {
                        let anchor=if rig.target.enabled {rig.target.clone()}
                            else if rig.position.enabled {let mut anchor=rig.position.clone();anchor.offset=[0.;3];anchor}
                            else {StudioCameraAttachment {lane,half_life:0.12,..Default::default()}};
                        if anchor.part!=StudioAttachmentPart::Player {self.render_studio.with_sabers=true;}
                        rig.target=anchor.clone();rig.target.enabled=true;
                        rig.position=StudioCameraAttachment {enabled:true,lane:anchor.lane,part:anchor.part,bone:anchor.bone,
                            local_offset:rig.position.local_offset,half_life:rig.position.half_life,..Default::default()};
                        match_view=Some(visible.position.to_array());
                    }
                    if ui.button("Bolt to saber").on_hover_text("Follow Saber 1's hilt with a rotating offset, aim at the head, and keep this shot").clicked() {
                        self.render_studio.with_sabers=true;
                        rig.position=StudioCameraAttachment {enabled:true,lane,part:StudioAttachmentPart::Saber1,
                            bone:"@hilt".into(),local_offset:true,half_life:0.08,..Default::default()};
                        rig.target=StudioCameraAttachment {enabled:true,lane,half_life:0.12,..Default::default()};
                        match_view=Some(visible.position.to_array());
                    }
                    if ui.add_enabled(rig.active(),egui::Button::new("Detach rig"))
                        .on_hover_text("Keep the displayed camera as a key at the playhead, then turn off both attachments").clicked() {detach=true;}
                });
                if self.render_studio.spatial.scene_camera.is_some() {
                    ui.horizontal_wrapped(|ui| {
                        ui.small("Scene view is an observer.");
                        if ui.button("Show shot").on_hover_text("Look through the camera controlled by this rig").clicked() {
                            self.render_studio.spatial.scene_camera=None;self.render_studio.spatial.drag=None;self.profile_preview_key=None;
                        }
                    });
                }
                for i in 0..2 {
                    ui.push_id(("camera_attachment",i),|ui| {
                      egui::Frame::group(ui.style()).inner_margin(8.).show(ui,|ui| {
                        ui.strong(if i==0 {"CAMERA POSITION"}else{"AIM TARGET"});
                        let binding=if i==0 {&mut rig.position}else{&mut rig.target};
                        let before=binding.clone();
                        ui.checkbox(&mut binding.enabled,if i==0 {"Follow position"}else{"Track target"});
                        if !binding.enabled {
                            ui.small(if i==0 {"Position comes from camera keys. Use Orbit target to follow and orbit a bone."}
                                else {"Direction comes from camera-key yaw and pitch."});return;
                        }
                        ui.horizontal(|ui| {
                          ui.label("Character");
                          egui::ComboBox::from_id_salt("layer").selected_text(format!("Layer {}",binding.lane+1))
                            .show_ui(ui,|ui|for lane in 0..self.render_studio.timeline.lanes {
                                ui.selectable_value(&mut binding.lane,lane,format!("Layer {}",lane+1));
                            });
                        });
                        let old_part=binding.part;
                        ui.horizontal(|ui| {
                          ui.label("Part");
                          egui::ComboBox::from_id_salt("part").selected_text(binding.part.label()).show_ui(ui,|ui| {
                            for part in [StudioAttachmentPart::Player,StudioAttachmentPart::Saber1,StudioAttachmentPart::Saber2] {
                                ui.selectable_value(&mut binding.part,part,part.label());
                            }
                          });
                        });
                        if old_part!=binding.part {
                            binding.bone=if binding.part==StudioAttachmentPart::Player {"*head_top"}else{"@hilt"}.into();
                            self.render_studio.camera_bone_search[i].clear();
                        }
                        if binding.part!=StudioAttachmentPart::Player {
                            ui.checkbox(&mut self.render_studio.with_sabers,"Attach selected sabers");
                        }
                        let info=self.studio_attachment_info(binding.lane);
                        let names=self.ensure_asset_preview_player_presenter().and_then(|_|self.asset_preview_player_presenter.as_mut()
                            .ok_or_else(||"Player presenter unavailable".to_owned())?
                            .profile_attachment_names(&info,binding.part.saber()));
                        ui.horizontal(|ui| {
                          ui.label("Bone");
                          egui::ComboBox::from_id_salt("bone").width(ui.available_width().min(240.))
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
                        ui.horizontal_wrapped(|ui| {
                            let quick: &[(&str,&str)]=if binding.part==StudioAttachmentPart::Player {
                                &[("Origin","@origin"),("Head","*head_top"),("R hand","*r_hand"),("L hand","*l_hand")]
                            } else {&[("Hilt","@hilt"),("Blade base","*blade1"),("Tip","@tip1")]};
                            for (label,name) in quick {if ui.small_button(*label).clicked(){binding.bone=(*name).into();}}
                        });
                        let runtime_binding=if i==0 {&previous.position}else{&previous.target};
                        let frame=if i==0 {edit_frame}else{self.render_studio.camera_rig_runtime.frames[i]};
                        let frame=frame.filter(|_|runtime_binding.enabled
                            && runtime_binding.lane==binding.lane && runtime_binding.part==binding.part && runtime_binding.bone==binding.bone);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(if i==0 {"Orbit space"}else{"Offset space"});
                            ui.selectable_value(&mut binding.local_offset,false,"World");
                            ui.add_enabled_ui(frame.is_none_or(|frame|
                                glam::Mat3::from_cols_array_2d(&frame.axis).determinant().abs()>1e-6),|ui| {
                                ui.selectable_value(&mut binding.local_offset,true,"Bone");
                            }).response.on_hover_text("If bone axes collapse during smoothing, reduce smoothing or use World space.");
                        }).response.on_hover_text("World: fixed directions, Y up. Bone: rotates with the attachment, local Z up. Switching space keeps the current position.");
                        // Changing spaces preserves the currently displayed attachment point.
                        if binding.local_offset!=before.local_offset {
                            if i==0 {convert_space=Some(binding.local_offset);}
                            if let Some(frame)=frame {
                                binding.offset=frame.offset(frame.point(before.offset,before.local_offset),binding.local_offset);
                            }
                        }
                        if i==0 {
                            ui.strong(edit_index.map_or_else(||format!("At {:.3}s — edit to add a camera key",edit_key.time),
                                |i|format!("Camera key {} · {:.3}s",i+1,edit_key.time)));
                            if let Some((frame,mut editor_binding,mut orbit))=frame.and_then(|frame| {
                                let point=previous.position_point(edit_key,frame);
                                let eye=std::array::from_fn(|a|point[a]+edit_key.position[a]-previous.reference_position[a]);
                                let mut editor_binding=binding.clone();editor_binding.offset=frame.offset(eye,binding.local_offset);
                                StudioCameraOrbit::from_binding(&editor_binding,frame,[0.;3]).map(|orbit|(frame,editor_binding,orbit))
                            }) {
                                let mut changed=false;
                                ui.horizontal(|ui| {
                                    ui.label("Distance");changed|=ui.add(egui::DragValue::new(&mut orbit.distance)
                                        .speed(1.).range(0.0..=50000.0).clamp_existing_to_range(false).max_decimals(2).suffix(" units")).changed();
                                }).response.on_hover_text("Distance at this camera key. It interpolates to the next key's distance. FOV changes framing separately.");
                                ui.add_enabled_ui(orbit.distance>0.001,|ui| {
                                    changed|=ui.add(egui::Slider::new(&mut orbit.yaw,-180.0..=180.0).text("Orbit yaw").suffix("°")).changed();
                                    changed|=ui.add(egui::Slider::new(&mut orbit.pitch,-90.0..=90.0).text("Orbit pitch").suffix("°")).changed();
                                });
                                if changed {orbit_edit=Some((edit_key.time,orbit));orbit.apply(&mut editor_binding,frame,[0.;3]);}
                                let offset_before=editor_binding.offset;
                                egui::CollapsingHeader::new("Advanced XYZ offset (this key)").id_salt("xyz_offset").show(ui,|ui| {
                                    offset_controls(ui,&mut editor_binding);
                                    if ui.button("Zero offset").clicked() {editor_binding.offset=[0.;3];}
                                });
                                if editor_binding.offset!=offset_before {
                                    if let Some(orbit)=StudioCameraOrbit::from_binding(&editor_binding,frame,[0.;3]) {orbit_edit=Some((edit_key.time,orbit));}
                                }
                            } else {ui.small("Orbit controls become available when the bone is ready. For unstable Bone axes, use World space or less smoothing.");}
                            ui.small(if binding.local_offset {"Orbit rotates with the bone."}else{"Orbit directions stay fixed in the world."});
                            ui.horizontal_wrapped(|ui| {
                                if ui.button("Use preview position").on_hover_text("Place the shot camera at the current preview camera position, including Scene view. Target tracking still controls aim.").clicked() {match_view=Some(preview_eye);}
                            });
                            ui.small("Distance, orbit angles and FOV animate between camera keys. Editing a selected key leaves the other keys unchanged.");
                            if !before.enabled || before.lane!=binding.lane || before.part!=binding.part || before.bone!=binding.bone {match_view=Some(visible.position.to_array());}
                        } else {
                            ui.label("Aim offset (whole track)");offset_controls(ui,binding);
                            if ui.small_button("Center on bone").clicked() {binding.offset=[0.;3];}
                            ui.small("The camera looks at this point automatically. Turn Track target off to use camera-key yaw/pitch.");
                        }
                        ui.add(egui::Slider::new(&mut binding.half_life,0.0..=2.0).text("Smoothing").suffix(" s"))
                            .on_hover_text("0 follows rigidly. Higher values soften movement with more lag. The value is the time to remove half the error; preview, scrubbing and exports match.");
                        ui.horizontal_wrapped(|ui|for (label,value) in [("Rigid",0.),("Gentle",0.12),("Smooth",0.3)] {
                            if ui.small_button(label).clicked() {binding.half_life=value;}
                        });
                        if let Some(message)=self.render_studio.camera_rig_runtime.messages[i].as_ref() {
                            ui.colored_label(egui::Color32::from_rgb(240,195,125),message);
                        }
                      });
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

fn offset_controls(ui:&mut egui::Ui,binding:&mut StudioCameraAttachment) {
    ui.horizontal_wrapped(|ui|for (a,label) in ["X ","Y ","Z "].into_iter().enumerate() {
        ui.add(egui::DragValue::new(&mut binding.offset[a]).speed(0.25).prefix(label).max_decimals(2));
    });
}
