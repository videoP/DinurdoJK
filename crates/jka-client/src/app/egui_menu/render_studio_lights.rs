//! Editable light rigs, source spans and transform/property keys.
use super::{App, render_studio_ui::{self as sui, palette as theme, ButtonKind, Icon}};
use crate::app::{render_studio_lights::*, render_studio_timeline::{StudioTrackRow, StudioInterpolation}};

pub(super) const LIGHT: egui::Color32 = egui::Color32::from_rgb(255, 207, 112);

impl App {
    /// Global ambient + directional sun for the isolated character stage.
    /// Keys are explicit: ordinary edits modify the chosen key; users add a
    /// key at the playhead when they actually want an animation.
    pub(super) fn egui_studio_scene_lighting_settings(&mut self,ui:&mut egui::Ui) {
        let time=self.render_studio.timeline.playhead;
        let smooth=self.render_studio.timeline.interpolation!=StudioInterpolation::Linear;
        let rig=&mut self.render_studio.timeline.scene_lighting;
        if rig.keys.is_empty(){rig.keys.push(StudioEnvironmentKey::default());}
        let index=self.render_studio.scene_lighting_selected_key.min(rig.keys.len()-1);
        self.render_studio.scene_lighting_selected_key=index;
        let mut key=rig.keys[index];
        sui::group_label(ui,"SCENE LIGHTING");
        sui::hint(ui,"Independent of gameplay lighting and BSP lightmaps. Applies to isolated character footage in both preview and export.");
        ui.separator();
        sui::group_label(ui,"Ambient / fill");
        sui::prop_row(ui,"Color",|ui|{ui.color_edit_button_rgb(&mut key.ambient_color);});
        sui::prop_drag(ui,"Intensity",egui::DragValue::new(&mut key.ambient_intensity)
            .range(0.0..=16.0).speed(0.02));
        ui.separator();
        sui::group_label(ui,"Directional sun");
        sui::prop_row(ui,"Color",|ui|{ui.color_edit_button_rgb(&mut key.sun_color);});
        sui::prop_drag(ui,"Intensity",egui::DragValue::new(&mut key.sun_intensity)
            .range(0.0..=16.0).speed(0.02));
        if sui::prop_vec3(ui,"Toward sun",&mut key.sun_direction,0.02,["X","Y","Z"],true) {
            for d in &mut key.sun_direction {*d=d.clamp(-1.0,1.0);}
            if glam::Vec3::from_array(key.sun_direction).length_squared()<0.000001 {
                key.sun_direction=rig.keys[index].sun_direction;
            }
        }
        sui::hint(ui,"Sun direction is the JKA-space vector toward the light. Directional illumination currently has no cast shadows.");
        if ui.button("Set ambient + sun to zero").clicked() {
            key.ambient_intensity=0.0;key.sun_intensity=0.0;
        }
        if key!=rig.keys[index] {
            // Never allow invalid keys into a saved project (including a
            // user-entered zero vector or non-finite drag value).
            if key.valid(){rig.keys[index]=key;self.profile_preview_key=None;}
        }
        ui.separator();
        sui::group_label(ui,"Output timeline keys");
        sui::hint(ui,&format!("Editing key at {:.2}s. It controls the scene until the next key. To animate lighting, add a key at the playhead.",rig.keys[index].time));
        if ui.add_enabled(rig.keys.len()<128,egui::Button::new("+ Key at playhead")).clicked(){
            let new_key=rig.sample(time,smooth);
            if let Some(i)=rig.add_key(new_key){self.render_studio.scene_lighting_selected_key=i;self.profile_preview_key=None;}
        }
        if ui.add_enabled(rig.keys.len()>1,egui::Button::new("Delete selected key")).clicked(){
            rig.keys.remove(index);
            self.render_studio.scene_lighting_selected_key=index.min(rig.keys.len()-1);
            self.profile_preview_key=None;
        }
        let mut seek_to=None;
        egui::ScrollArea::vertical().max_height(116.0).show(ui,|ui|{
            for (i,k) in rig.keys.iter().enumerate(){
                if ui.selectable_label(i==self.render_studio.scene_lighting_selected_key,
                    format!("◇  {:.2}s     Ambient {:.2} · Sun {:.2}",k.time,k.ambient_intensity,k.sun_intensity)).clicked(){
                    self.render_studio.scene_lighting_selected_key=i;
                    seek_to=Some(k.time);
                }
            }
        });
        if let Some(t)=seek_to {self.render_studio.timeline.seek(t);self.profile_preview_key=None;}
        sui::hint(ui,"Ambient 0 + Sun 0 + no active Point/Spot lights = black lit surfaces. Unlit/emissive materials can still glow.");
    }

    pub(super) fn egui_studio_light_overlay(&mut self,ui:&mut egui::Ui,viewport:egui::Rect,locked:bool)->bool {
        if locked {return false;}
        let time=self.render_studio.timeline_time();
        let camera=self.render_studio.scene_frame.as_ref().map_or_else(||self.asset_preview_camera(),|f|f.outgoing.camera);
        let project=|p:[f32;3]|camera.project_to_screen(viewport.width().max(1.0) as u32,viewport.height().max(1.0) as u32,
            glam::Vec3::from_array(crate::scene::render_position(p))).map(|p|viewport.min+p.to_vec2());
        let mut points=Vec::new();
        for (layer,group) in self.render_studio.timeline.light_layers.iter().enumerate().filter(|(_,l)|l.enabled){
            for (i,source) in group.lights.iter().enumerate().filter(|(_,l)|l.enabled&&time>=l.start&&time<l.end()){
                let key=source.sample(time,self.render_studio.timeline.interpolation!=StudioInterpolation::Linear);
                points.push((layer,i,source.kind,key));
            }
        }
        let painter=ui.painter().with_clip_rect(viewport);
        let mut busy=false;
        for (layer,i,kind,key) in points {
            let selected=self.render_studio.light_ui.selection.is_some_and(|s|s.layer==layer&&s.light==Some(i));
            let aim=key.aim_handle();
            if kind==StudioLightKind::Spot {if let (Some(a),Some(b))=(project(key.position),project(aim)) {
                painter.line_segment([a,b],egui::Stroke::new(1.5,LIGHT));
            }}
            let handles=std::iter::once((false,key.position)).chain((kind==StudioLightKind::Spot).then_some((true,aim)));
            for (target,point) in handles {
                let Some(center)=project(point).filter(|p|viewport.shrink(12.0).contains(*p)) else{continue};
                painter.circle_stroke(center,if selected{8.0}else{6.0},egui::Stroke::new(1.5,LIGHT));
                painter.line_segment([center-egui::vec2(11.0,0.0),center+egui::vec2(11.0,0.0)],egui::Stroke::new(1.0,LIGHT));
                painter.line_segment([center-egui::vec2(0.0,11.0),center+egui::vec2(0.0,11.0)],egui::Stroke::new(1.0,LIGHT));
                if selected {painter.text(center+egui::vec2(11.0,11.0),egui::Align2::LEFT_TOP,if target{"Aim"}else{"Light"},egui::FontId::proportional(11.0),LIGHT);}
                let r=ui.interact(egui::Rect::from_center_size(center,egui::vec2(24.0,24.0)),ui.make_persistent_id(("studio_light_gizmo",layer,i,target)),egui::Sense::click_and_drag());
                busy|=r.hovered()||r.is_pointer_button_down_on();
                let selection=StudioLightSelection{layer,light:Some(i),key:None};
                if r.clicked(){self.studio_select_light(selection);}
                if r.drag_started(){
                    self.studio_select_light(selection);self.render_studio.timeline.playing=false;
                    self.render_studio.light_ui.position_drag=Some(StudioLightHandleDrag{selection,target,key,
                        spatial:crate::app::render_studio_spatial::StudioSpatialDrag{camera,viewport,
                            origin:glam::Vec3::from_array(crate::scene::render_position(point)),
                            start_pointer:ui.ctx().input(|i|i.pointer.press_origin()).unwrap_or(center),axis:None,items:vec![]}});
                }
                r.on_hover_text(if target {"Spotlight aim. The handle stays at a fixed length; light range is unchanged."}
                    else {"Light position. Drag to key position; the spotlight aim moves with the light."});
            }
        }
        if let Some(drag)=self.render_studio.light_ui.position_drag.clone() {
            busy=true;
            if let Some(delta)=ui.ctx().input(|i|i.pointer.interact_pos()).and_then(|p|drag.spatial.delta(p)) {
                let point=crate::scene::jka_position((drag.spatial.origin+delta).to_array());
                let mut key=drag.key;
                if drag.target {key.aim_at(point);}else{key.move_position(point);}
                if let Some(source)=self.render_studio.timeline.light_layers.get_mut(drag.selection.layer)
                    .and_then(|l|l.lights.get_mut(drag.selection.light.unwrap_or(usize::MAX))) {
                    if let Some(k)=source.add_key(key){self.render_studio.light_ui.selection=Some(StudioLightSelection{key:Some(k),..drag.selection});}
                }
                self.profile_preview_key=None;ui.ctx().request_repaint();
            }
        }
        if !ui.ctx().input(|i|i.pointer.primary_down()){self.render_studio.light_ui.position_drag=None;}
        busy
    }
    pub(super) fn studio_select_light(&mut self, selection: StudioLightSelection) {
        self.render_studio.timeline.clear_selection();
        self.render_studio.director_selected=None;
        self.render_studio.selected_background=false;
        self.render_studio.audio.selected=None;self.render_studio.audio.selected_track=None;
        self.render_studio.audio.selected_demo_audio=None;
        self.render_studio.light_ui.selection=Some(selection);
    }
    pub(super) fn studio_sync_light_selection(&mut self) {
        let studio=&mut self.render_studio;
        if !studio.timeline.selected_items().is_empty() || studio.timeline.selected_demo.is_some()
            || studio.director_selected.is_some() || studio.selected_background
            || studio.audio.selected.is_some() || studio.audio.selected_track.is_some() || studio.audio.selected_demo_audio.is_some() {
            studio.light_ui=Default::default();
        }
    }
    pub(super) fn studio_add_light_layer(&mut self) {
        let layers=&mut self.render_studio.timeline.light_layers;
        if layers.len()>=MAX_LAYERS {return;}
        let layer=layers.len();layers.push(StudioLightLayer::new(layer+1));
        self.studio_select_light(StudioLightSelection{layer,light:None,key:None});
    }
    fn studio_add_light(&mut self,layer:usize,kind:StudioLightKind) {
        if self.render_studio.timeline.light_layers.iter().map(|l|l.lights.len()).sum::<usize>()>=MAX_LIGHTS {return;}
        let time=self.render_studio.timeline_time().min(35999.0);
        let duration=(self.render_studio.timeline.duration()-time).max(5.0).min(36000.0-time);
        let camera=self.render_studio.scene_frame.as_ref().map_or_else(||self.asset_preview_camera(),|s|s.outgoing.camera);
        let position=camera.position+camera.forward()*64.0+glam::Vec3::Y*32.0;
        let target=camera.position+camera.forward()*220.0;
        let key=StudioLightKey {position:crate::scene::jka_position(position.to_array()),
            target:crate::scene::jka_position(target.to_array()),..Default::default()};
        let Some(group)=self.render_studio.timeline.light_layers.get_mut(layer) else{return};
        let light=group.lights.len();group.expanded=true;
        group.lights.push(StudioLight{name:format!("{} {}",kind.label(),light+1),kind,
            emitter:StudioLightEmitter::default(),enabled:true,start:time,duration,keys:vec![key]});
        self.studio_select_light(StudioLightSelection{layer,light:Some(light),key:Some(0)});
        self.profile_preview_key=None;
    }
    pub(super) fn studio_add_light_key(&mut self,layer:usize,light:usize,time:f64) {
        let smooth=self.render_studio.timeline.interpolation!=StudioInterpolation::Linear;
        let Some(source)=self.render_studio.timeline.light_layers.get_mut(layer).and_then(|l|l.lights.get_mut(light)) else{return};
        let key=source.sample(time,smooth);
        let key=source.add_key(key);
        self.studio_select_light(StudioLightSelection{layer,light:Some(light),key});
        self.profile_preview_key=None;
    }
    pub(super) fn studio_delete_light_selection(&mut self)->bool {
        let Some(selection)=self.render_studio.light_ui.selection else{return false};
        let Some(layer)=self.render_studio.timeline.light_layers.get_mut(selection.layer) else{return false};
        if let Some(i)=selection.light {
            if let Some(light)=layer.lights.get_mut(i) {
                if let Some(k)=selection.key {
                    if light.keys.len()>1 && k<light.keys.len() {light.keys.remove(k);}
                    self.render_studio.light_ui.selection=Some(StudioLightSelection{key:None,..selection});
                } else {layer.lights.remove(i);self.render_studio.light_ui.selection=None;}
            }
        } else {self.render_studio.timeline.light_layers.remove(selection.layer);self.render_studio.light_ui.selection=None;}
        self.profile_preview_key=None;true
    }
    pub(super) fn egui_studio_light_header(&mut self,ui:&mut egui::Ui,p:&egui::Painter,head:egui::Rect,kind:StudioTrackRow,locked:bool) {
        let (layer,light)=match kind {StudioTrackRow::LightLayer(l)=>(l,None),StudioTrackRow::Light(l,i)=>(l,Some(i)),_=>return};
        let selected=self.render_studio.light_ui.selection.is_some_and(|s|s.layer==layer&&s.light==light);
        if selected {p.rect_stroke(head.shrink(1.0),2.0,egui::Stroke::new(1.0,LIGHT),egui::StrokeKind::Inside);}
        let offset=if light.is_none(){54.0}else{26.0};
        let click=ui.interact(egui::Rect::from_min_max(head.min+egui::vec2(offset,0.0),head.max),
            ui.make_persistent_id(("studio_light_header",layer,light)),egui::Sense::click());
        if !locked&&click.clicked(){self.studio_select_light(StudioLightSelection{layer,light,key:None});}
        if light.is_none() {
            let expanded=self.render_studio.timeline.light_layers[layer].expanded;
            let visible=self.render_studio.timeline.light_layers[layer].enabled;
            let chevron=egui::Rect::from_center_size(head.left_center()+egui::vec2(24.0,0.0),egui::vec2(18.0,22.0));
            let eye=egui::Rect::from_center_size(head.left_center()+egui::vec2(45.0,0.0),egui::vec2(18.0,22.0));
            sui::paint_icon(p,chevron.center(),if expanded{Icon::ChevronDown}else{Icon::ChevronRight},theme::TEXT_DIM);
            sui::paint_icon(p,eye.center(),if visible{Icon::Eye}else{Icon::EyeOff},theme::TEXT_DIM);
            if !locked {
                if ui.interact(chevron,ui.make_persistent_id(("studio_light_expand",layer)),egui::Sense::click()).clicked(){
                    self.render_studio.timeline.light_layers[layer].expanded=!expanded;
                }
                if ui.interact(eye,ui.make_persistent_id(("studio_light_eye",layer)),egui::Sense::click()).on_hover_text("Toggle the entire light rig").clicked(){
                    self.render_studio.timeline.light_layers[layer].enabled=!visible;self.profile_preview_key=None;
                }
            }
        }
    }
    pub(super) fn egui_studio_light_timeline(&mut self,ui:&mut egui::Ui,p:&egui::Painter,draw:egui::Rect,x0:f32,right:f32,ruler_h:f32,lane_h:f32,zoom:f32,scroll:f32,locked:bool) {
        let mut sources=Vec::new();
        for (layer,group) in self.render_studio.timeline.light_layers.iter().enumerate() {
            if !group.expanded {continue;}
            for (i,light) in group.lights.iter().enumerate(){
                if let Some(row)=self.render_studio.timeline.light_row(layer,i){sources.push((layer,i,row,group.enabled,light.clone()));}
            }
        }
        let to_x=|t:f64|x0+(t as f32-scroll)*zoom;
        for (layer,i,row,group_enabled,light) in sources {
            let top=draw.top()+ruler_h+row as f32*lane_h;
            let lane=egui::Rect::from_min_max(egui::pos2(x0,top),egui::pos2(right,top+lane_h));
            let paint=p.with_clip_rect(lane.intersect(p.clip_rect()));
            let block=egui::Rect::from_min_max(egui::pos2(to_x(light.start).max(x0),top+4.0),egui::pos2(to_x(light.end()).min(right),top+lane_h-4.0));
            if block.width()<=0.0 {continue;}
            let selected=self.render_studio.light_ui.selection.is_some_and(|s|s.layer==layer&&s.light==Some(i));
            super::render_studio::studio_paint_clip(&paint,block,if group_enabled&&light.enabled{LIGHT}else{theme::TEXT_DISABLED},selected,&light.name,light.kind.label());
            if !locked {
                let response=ui.interact(block,ui.make_persistent_id(("studio_light_span",layer,i)),egui::Sense::click_and_drag());
                if response.clicked(){self.studio_select_light(StudioLightSelection{layer,light:Some(i),key:None});}
                if response.double_clicked(){if let Some(point)=response.interact_pointer_pos(){
                    self.studio_add_light_key(layer,i,f64::from((point.x-x0)/zoom+scroll));
                }}
                if response.drag_started(){
                    let selection=StudioLightSelection{layer,light:Some(i),key:None};self.studio_select_light(selection);
                    self.render_studio.light_ui.drag=Some(StudioLightDrag{selection,pointer_x:response.interact_pointer_pos().map_or(block.left(),|p|p.x),scroll,initial:light.start});
                }
            }
            for (key_index,key) in light.keys.iter().enumerate(){
                let time=light.start+key.time;if time>light.end(){continue;}
                let center=egui::pos2(to_x(time),top+lane_h*0.5);
                if center.x<x0||center.x>right {continue;}
                let selected_key=selected&&self.render_studio.light_ui.selection.is_some_and(|s|s.key==Some(key_index));
                paint.add(egui::Shape::convex_polygon(vec![center+egui::vec2(0.0,-7.0),center+egui::vec2(7.0,0.0),center+egui::vec2(0.0,7.0),center+egui::vec2(-7.0,0.0)],
                    if selected_key{egui::Color32::WHITE}else{LIGHT},egui::Stroke::new(1.0,theme::SURFACE)));
                if !locked {
                    let response=ui.interact(egui::Rect::from_center_size(center,egui::vec2(17.0,20.0)).intersect(lane),
                        ui.make_persistent_id(("studio_light_key",layer,i,key_index)),egui::Sense::click_and_drag());
                    let selection=StudioLightSelection{layer,light:Some(i),key:Some(key_index)};
                    if response.clicked(){self.studio_select_light(selection);self.render_studio.timeline.seek(time);self.profile_preview_key=None;}
                    if response.drag_started(){self.studio_select_light(selection);self.render_studio.light_ui.drag=Some(StudioLightDrag{
                        selection,pointer_x:response.interact_pointer_pos().map_or(center.x,|p|p.x),scroll,initial:key.time});}
                    response.on_hover_text(format!("Light key at {time:.2}s · drag to retime · edit position and properties in Inspector"));
                }
            }
        }
        if let Some(drag)=self.render_studio.light_ui.drag {
            if !ui.ctx().input(|i|i.pointer.primary_down()){self.render_studio.light_ui.drag=None;}
            else if let Some(point)=ui.ctx().input(|i|i.pointer.interact_pos()){
                let delta=f64::from((point.x-drag.pointer_x)/zoom+scroll-drag.scroll);
                if let Some(light)=self.render_studio.timeline.light_layers.get_mut(drag.selection.layer)
                    .and_then(|l|l.lights.get_mut(drag.selection.light?)) {
                    if let Some(k)=drag.selection.key.filter(|k|*k<light.keys.len()){
                        let lo=if k==0{0.0}else{light.keys[k-1].time+0.001};
                        let hi=light.keys.get(k+1).map_or(light.duration,|v|v.time-0.001);
                        if hi>=lo{light.keys[k].time=(drag.initial+delta).clamp(lo,hi);}
                    }else{light.start=(drag.initial+delta).clamp(0.0,36000.0-light.duration);}
                }
                self.profile_preview_key=None;ui.ctx().request_repaint();
            }
        }
    }
    pub(super) fn egui_studio_light_inspector(&mut self,ui:&mut egui::Ui)->bool {
        let Some(selection)=self.render_studio.light_ui.selection else{return false};
        let Some(group)=self.render_studio.timeline.light_layers.get(selection.layer) else{self.render_studio.light_ui.selection=None;return false};
        if let Some(i)=selection.light {
            let Some(mut source)=group.lights.get(i).cloned() else{self.render_studio.light_ui.selection=None;return false};
            sui::inspector_title(ui,LIGHT,"Light",&source.name,"Independent keys on the output timeline");
            ui.text_edit_singleline(&mut source.name);
            ui.checkbox(&mut source.enabled,"Enabled");
            sui::prop_combo(ui,"Type",("studio_light_kind",selection.layer,i),&mut source.kind,&[(StudioLightKind::Point,"Point"),(StudioLightKind::Spot,"Spot")]);
            let old_emitter=source.emitter.kind;
            sui::prop_combo(ui,"Emitter type",("studio_light_emitter",selection.layer,i),&mut source.emitter.kind,&[
                (StudioLightEmitterKind::Continuous,"Continuous"),
                (StudioLightEmitterKind::Pulse,"Pulse (sine)"),
                (StudioLightEmitterKind::Triangle,"Triangle"),
                (StudioLightEmitterKind::Strobe,"Strobe (square)"),
                (StudioLightEmitterKind::Sawtooth,"Sawtooth"),
                (StudioLightEmitterKind::InverseSawtooth,"Inverse sawtooth"),
                (StudioLightEmitterKind::Noise,"Smooth noise"),
                (StudioLightEmitterKind::Flicker,"Flicker (stepped)"),
            ]);
            if old_emitter != source.emitter.kind && old_emitter == StudioLightEmitterKind::Continuous {
                source.emitter.frequency=match source.emitter.kind {
                    StudioLightEmitterKind::Strobe=>4.0,
                    StudioLightEmitterKind::Noise|StudioLightEmitterKind::Flicker=>12.0,
                    _=>1.0,
                };
            }
            if source.emitter.kind != StudioLightEmitterKind::Continuous {
                sui::prop_drag(ui,"Rate",egui::DragValue::new(&mut source.emitter.frequency).range(0.05..=60.0).speed(0.05).suffix(" Hz"));
                sui::prop_drag(ui,"Phase",egui::DragValue::new(&mut source.emitter.phase).range(0.0..=1.0).speed(0.005).fixed_decimals(3));
                if source.emitter.kind==StudioLightEmitterKind::Strobe {
                    let mut duty=source.emitter.duty_cycle*100.0;
                    sui::prop_drag(ui,"On time",egui::DragValue::new(&mut duty).range(1.0..=99.0).speed(0.5).suffix(" %"));
                    source.emitter.duty_cycle=(duty/100.0).clamp(0.01,0.99);
                }
                let mut floor=source.emitter.floor*100.0;
                sui::prop_drag(ui,"Minimum output",egui::DragValue::new(&mut floor).range(0.0..=100.0).speed(0.5).suffix(" %"));
                source.emitter.floor=(floor/100.0).clamp(0.0,1.0);
                sui::hint(ui,"Emitter timing uses seconds from this light's start, not demo time. Phase is 0–1 cycles; 0% minimum allows full darkness. Keyed intensity still controls peak output.");
            }
            sui::prop_drag(ui,"Start",egui::DragValue::new(&mut source.start).range(0.0..=35999.0).speed(0.05).suffix(" s"));
            sui::prop_drag(ui,"Duration",egui::DragValue::new(&mut source.duration).range(0.01..=36000.0-source.start).speed(0.05).suffix(" s"));
            let now=self.render_studio.timeline_time();
            let smooth=self.render_studio.timeline.interpolation!=StudioInterpolation::Linear;
            let mut key=selection.key.and_then(|k|source.keys.get(k).copied()).unwrap_or_else(||source.sample(now,smooth));
            let old=key;
            sui::hint(ui,&format!("{} · {:.2}s from light start",if selection.key.is_some(){"Selected key"}else{"Editing at playhead (auto key)"},key.time));
            sui::prop_vec3(ui,"Position",&mut key.position,1.0,["X","Y","Z"],true);
            sui::prop_row(ui,"Color",|ui|{ui.color_edit_button_rgb(&mut key.color);});
            sui::prop_drag(ui,"Intensity",egui::DragValue::new(&mut key.intensity).range(0.0..=64.0).speed(0.05));
            sui::prop_drag(ui,"Range",egui::DragValue::new(&mut key.radius).range(1.0..=65536.0).speed(2.0));
            sui::prop_drag(ui,"Emitter size",egui::DragValue::new(&mut key.source_radius).range(0.0..=key.radius).speed(0.5));
            if source.kind==StudioLightKind::Spot {
                let mut direction=(glam::Vec3::from_array(key.aim_handle())-glam::Vec3::from_array(key.position)).normalize_or_zero().to_array();
                if sui::prop_vec3(ui,"Aim direction",&mut direction,0.01,["X","Y","Z"],true) {
                    key.aim_at((glam::Vec3::from_array(key.position)+glam::Vec3::from_array(direction)).to_array());
                }
                sui::prop_drag(ui,"Outer cone",egui::DragValue::new(&mut key.outer_angle).range(1.0..=89.0).suffix("°").speed(0.25));
                sui::prop_drag(ui,"Inner cone",egui::DragValue::new(&mut key.inner_angle).range(0.0..=key.outer_angle).suffix("°").speed(0.25));
            }
            if key.position!=old.position && key.target==old.target {let position=key.position;key.position=old.position;key.move_position(position);}
            if source.kind==StudioLightKind::Spot && key!=old {key.aim_at(key.target);}
            key.inner_angle=key.inner_angle.min(key.outer_angle);key.source_radius=key.source_radius.min(key.radius);
            if sui::button(ui,Some(Icon::Camera),"Place at view",ButtonKind::Normal,true).clicked(){
                let camera=self.render_studio.scene_frame.as_ref().map_or_else(||self.asset_preview_camera(),|s|s.outgoing.camera);
                key.position=crate::scene::jka_position(camera.position.to_array());
                key.target=crate::scene::jka_position((camera.position+camera.forward()*220.0).to_array());
            }
            if key!=old {if let Some(k)=source.add_key(key){self.render_studio.light_ui.selection=Some(StudioLightSelection{key:Some(k),..selection});}}
            self.render_studio.timeline.light_layers[selection.layer].lights[i]=source;
            if sui::button(ui,Some(Icon::AddKey),"Key at playhead",ButtonKind::Normal,true).clicked(){self.studio_add_light_key(selection.layer,i,now);}
            ui.horizontal(|ui|{
                if ui.add_enabled(selection.key.is_some()&&self.render_studio.timeline.light_layers[selection.layer].lights[i].keys.len()>1,egui::Button::new("Delete key")).clicked(){self.studio_delete_light_selection();}
                if ui.button("Delete light").clicked(){self.render_studio.light_ui.selection=Some(StudioLightSelection{key:None,..selection});self.studio_delete_light_selection();}
            });
        } else {
            let name=group.name.clone();sui::inspector_title(ui,LIGHT,"Light layer",&name,"One rig can contain multiple independent lights");
            let layer=&mut self.render_studio.timeline.light_layers[selection.layer];
            ui.text_edit_singleline(&mut layer.name);ui.checkbox(&mut layer.enabled,"Enable this rig");
            let lights:Vec<_>=layer.lights.iter().map(|l|l.name.clone()).collect();
            for (i,name) in lights.iter().enumerate(){if ui.button(name).clicked(){self.studio_select_light(StudioLightSelection{layer:selection.layer,light:Some(i),key:None});}}
            ui.horizontal(|ui|{
                if ui.button("+ Point").clicked(){self.studio_add_light(selection.layer,StudioLightKind::Point);}
                if ui.button("+ Spot").clicked(){self.studio_add_light(selection.layer,StudioLightKind::Spot);}
            });
            if ui.button("Delete light layer").clicked(){self.render_studio.light_ui.selection=Some(selection);self.studio_delete_light_selection();}
        }
        true
    }
}
