//! Render Studio presents both the isolated Ghoul2 animation stage and native
//! demo-world playback in the same timeline-controlled preview panel.
//!
//! Layout follows NLE conventions: media browser (left), program viewer with
//! its own transport bar (center), inspector (right), timeline dock (bottom).
//! Taffy computes every pane part; `render_studio_ui` paints the chrome.
use super::App;
use super::render_studio_ui::palette as theme;
use crate::app::render_studio::{StudioBackground, StudioVideoFormat, StudioMediaTab, StudioMediaDrag, StudioExportSize, StudioEncoderSpeed};
use super::render_studio_layout;
use super::render_studio_spatial::studio_layer_color;
use super::render_studio_ui::{self as sui, ButtonKind, Icon};
use crate::app::render_studio_timeline::{StudioItem,StudioDemoClip};

/// The same choices appear for a clip's own trail and each composite actor.
fn studio_trail_label_picker(ui:&mut egui::Ui,id:impl std::hash::Hash + std::fmt::Debug,
    value:&mut crate::strafe_trail::StudioTickLabels) {
    use crate::strafe_trail::StudioTickLabels as Format;
    ui.horizontal(|ui| {
        ui.label("Tick time labels");
        egui::ComboBox::from_id_salt(("studio_tick_labels",egui::Id::new(&id)))
            .selected_text(match *value {Format::Off=>"Off",Format::Seconds=>"Seconds",Format::MinutesSeconds=>"Minutes:seconds"})
            .show_ui(ui,|ui| {
                ui.selectable_value(value,Format::Off,"Off");
                ui.selectable_value(value,Format::Seconds,"Seconds (12)");
                ui.selectable_value(value,Format::MinutesSeconds,"Minutes:seconds (0:12)");
            });
    });
}
/// Font and sizes of the ghost race standings and name tags. Installed
/// fonts are scanned on a worker the first time the picker opens.
fn studio_overlay_style_section(ui:&mut egui::Ui,style:&mut crate::app::render_studio_race::RaceOverlayStyle,
    fonts:&mut Option<Vec<(String,String)>>,job:&mut Option<std::thread::JoinHandle<Vec<(String,String)>>>,filter:&mut String) {
    if job.as_ref().is_some_and(|j|j.is_finished()) {
        *fonts=Some(job.take().unwrap().join().unwrap_or_default());
    }
    sui::section(ui,"studio_ghost_race_overlay","Overlay text",
        Some("Standings and name tags are drawn into preview and export with this font. Default uses fonts/studio_overlay.ttf from your pk3s when present, else Segoe UI Semibold."),true,|ui| {
        let current=if style.font.is_empty() {"Default".to_owned()}
            else if style.font==crate::ui::STUDIO_FONT_CLASSIC {"Classic JKA (ergoec)".to_owned()}
            else {fonts.as_ref().and_then(|f|f.iter().find(|(_,p)|*p==style.font)).map_or_else(
                ||std::path::Path::new(&style.font).file_name().map_or(style.font.clone(),|n|n.to_string_lossy().into_owned()),
                |(name,_)|name.clone())};
        ui.horizontal(|ui| {
            ui.label("Font");
            let response=egui::ComboBox::from_id_salt("studio_overlay_font").selected_text(current).width(220.0)
                .height(320.0).show_ui(ui,|ui| {
                    ui.add(egui::TextEdit::singleline(filter).hint_text("Search fonts"));
                    ui.selectable_value(&mut style.font,String::new(),"Default");
                    ui.selectable_value(&mut style.font,crate::ui::STUDIO_FONT_CLASSIC.to_owned(),"Classic JKA (ergoec)");
                    ui.separator();
                    match fonts.as_ref() {
                        None=>{ui.label("Reading installed fonts…");}
                        Some(list)=>{
                            let needle=filter.to_lowercase();
                            for (name,path) in list.iter().filter(|(n,_)|needle.is_empty()||n.to_lowercase().contains(&needle)) {
                                ui.selectable_value(&mut style.font,path.clone(),name.as_str()).on_hover_text(path.as_str());
                            }
                        }
                    }
                });
            if response.response.clicked() && fonts.is_none() && job.is_none() {
                *job=std::thread::Builder::new().name("studio-font-scan".into())
                    .spawn(crate::ui::list_system_fonts).ok();
            }
            if job.is_some() {ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));}
        });
        sui::prop_slider(ui,"Standings size",egui::Slider::new(&mut style.readout_scale,0.5..=2.0));
        sui::prop_slider(ui,"Name tag size",egui::Slider::new(&mut style.label_scale,0.4..=1.5));
        sui::prop_slider(ui,"Ghost name opacity",egui::Slider::new(&mut style.label_alpha,0.1..=1.0));
    });
}
fn studio_trail_color_picker(ui:&mut egui::Ui,auto_color:[u8;3],color:&mut Option<[u8;3]>) {
    ui.horizontal(|ui| {
        ui.label("Trail color");
        let mut automatic=color.is_none();
        if ui.checkbox(&mut automatic,"Name-seeded").changed() {
            *color=if automatic {None} else {Some(auto_color)};
        }
        let mut rgb=color.unwrap_or(auto_color);
        if automatic {
            ui.colored_label(egui::Color32::from_rgb(rgb[0],rgb[1],rgb[2]),"●");
        } else if ui.color_edit_button_srgb(&mut rgb).changed() {
            *color=Some(rgb);
        }
    });
}

fn bounds_rect(root:egui::Pos2,b:render_studio_layout::Bounds)->egui::Rect {
    egui::Rect::from_min_max(root+egui::vec2(b[0],b[1]),root+egui::vec2(b[2],b[3]))
}

/// Collapsed side pane: a slim rail with a rotated caption. Any click expands.
fn collapsed_rail(ui:&mut egui::Ui,rect:egui::Rect,title:&str,expand_icon:Icon,id:&str)->bool {
    let r=ui.interact(rect,ui.make_persistent_id(id),egui::Sense::click());
    let p=ui.painter();
    p.rect_filled(rect,2.0,if r.hovered(){theme::CONTROL}else{sui::PANE});
    sui::paint_icon(p,egui::pos2(rect.center().x,rect.top()+15.0),expand_icon,
        if r.hovered(){theme::TEXT}else{theme::TEXT_DIM});
    let galley=p.layout_no_wrap(title.to_owned(),egui::FontId::proportional(11.5),theme::TEXT_FAINT);
    let pos=egui::pos2(rect.center().x+galley.size().y*0.5,rect.top()+38.0);
    p.add(egui::epaint::TextShape::new(pos,galley,theme::TEXT_FAINT).with_angle(std::f32::consts::FRAC_PI_2));
    r.on_hover_text(format!("Expand {}",title.to_ascii_lowercase())).clicked()
}

fn format_seconds(seconds:f64)->String {
    if seconds>=60.0 {format!("{}:{:04.1}",(seconds/60.0) as u32,seconds%60.0)} else {format!("{seconds:.1}s")}
}

pub(super) fn studio_background_choices(ui:&mut egui::Ui,value:&mut StudioBackground)->[egui::Response;5] {
    [(StudioBackground::Black,"Empty / transparent"),(StudioBackground::Green,"Green screen"),
        (StudioBackground::Blue,"Blue screen"),(StudioBackground::Levelshot,"Map levelshot (2D)"),
        (StudioBackground::Bsp,"BSP world (3D)")]
        .map(|(background,label)|ui.selectable_value(value,background,label))
}

/// Studio supports the native, renderer-owned HUD elements for which the
/// gameplay HUD editor already has a rectangle/transform. No duplicate 2D HUD
/// is composited over final exports: these handles are editor-only.
const STUDIO_HUD_ITEMS: &[(crate::ui::HudElementId,&str)] = &[
    (crate::ui::HudElementId::Health,"core"),
    (crate::ui::HudElementId::Shield,"core"),
    (crate::ui::HudElementId::Ammo,"core"),
    (crate::ui::HudElementId::Force,"core"),
    (crate::ui::HudElementId::Score,"score"),
    (crate::ui::HudElementId::MiniScores,"mini_scores"),
    (crate::ui::HudElementId::Fps,"game_timer"),
    // Studio-only interpretation: move the native crosshair, not target text.
    (crate::ui::HudElementId::CrosshairName,"crosshair"),
    (crate::ui::HudElementId::EnemyInfo,"enemy_info"),
    (crate::ui::HudElementId::RaceTimer,"race_timer"),
    (crate::ui::HudElementId::Speedometer,"speedometer"),
    (crate::ui::HudElementId::MovementKeys,"movement_keys"),
    (crate::ui::HudElementId::StrafeGuide,"strafe_helper"),
    (crate::ui::HudElementId::Follow,"follow"),
];

/// HUD geometry in Studio's persisted 1280x720 reference coordinates.
/// Use this same rectangle for grid lines, movement, and scale quantization;
/// preview point dimensions depend on the size of the Program Preview pane.
fn studio_hud_reference_rect(
    id: crate::ui::HudElementId, layout: crate::ui::HudElementLayout,
    ctx: &crate::ui::HudRectContext, w: u32, h: u32,
) -> crate::ui::HudRect {
    let mut native_layout = layout;
    native_layout.offset[0] *= w as f32 / 1280.0;
    native_layout.offset[1] *= h as f32 / 720.0;
    let mut r = crate::ui::hud_element_rect(id, native_layout, ctx, w, h);
    if id == crate::ui::HudElementId::CrosshairName {
        r.x = w as f32 * 0.5 + native_layout.offset[0] - 22.0 * layout.scale;
        r.y = h as f32 * 0.5 + native_layout.offset[1] - 22.0 * layout.scale;
        r.width = 44.0 * layout.scale;
        r.height = 44.0 * layout.scale;
    }
    crate::ui::HudRect {
        x: r.x * 1280.0 / w as f32,
        y: r.y * 720.0 / h as f32,
        width: r.width * 1280.0 / w as f32,
        height: r.height * 720.0 / h as f32,
    }
}

fn studio_snap_position(
    id: crate::ui::HudElementId, mut layout: crate::ui::HudElementLayout,
    scene: &crate::ui::HudLayout, ctx: &crate::ui::HudRectContext,
    w: u32, h: u32, grid: f32,
    enabled: impl Fn(&str) -> bool,
) -> crate::ui::HudElementLayout {
    let proposed = studio_hud_reference_rect(id, layout, ctx, w, h);
    let neighbors = STUDIO_HUD_ITEMS.iter().copied()
        .filter(|(other, key)| *other != id && enabled(key))
        .map(|(other, _)| studio_hud_reference_rect(other, scene.element(other), ctx, w, h));
    let (point, _) = crate::ui::snap_hud_rect_position(proposed, neighbors, grid, 12.0, 32.0);
    layout.offset[0] += point[0] - proposed.x;
    layout.offset[1] += point[1] - proposed.y;
    layout
}

impl App {
    /// Snap visible Studio element positions on preview entry and reset,
    /// while preserving any deliberately chosen Studio CFG scale.
    pub(super) fn align_studio_defaults_for_editor(&mut self) {
        let (w,h)=self.window.as_ref().map(|window| {
            let size=window.inner_size();(size.width.max(1),size.height.max(1))
        }).unwrap_or((1280,720));
        let video=crate::ui::VideoSettings{draw_fps:0,draw_timer:false,..self.video};
        let ctx=crate::ui::HudRectContext::new(self.movement_keys_hud,&video,&self.perf,
            self.threads.len(),&self.japro_cg,self.strafe_helper,false,[0.,0.],2);
        let defaults=crate::ui::HudLayout::default();
        let grid=self.render_studio.hud_layout.grid_size.clamp(1.,64.);
        if !self.render_studio.hud_layout.snap_to_grid {return;}
        for &(id,_) in STUDIO_HUD_ITEMS {
            let original=self.render_studio.hud_layout.element(id);
            let mut candidate=original;
            // A default scale is quantized once; custom sizing stays intact.
            // Either way the element's visible bounds begin on the 8-unit grid.
            if original==defaults.element(id) {
                let mut unit=candidate;unit.scale=1.;
                let stock=studio_hud_reference_rect(id,unit,&ctx,w,h);
                candidate.scale=crate::ui::snap_hud_dimensions_scale(
                    [stock.width,stock.height],candidate.scale,grid);
            }
            let rect=studio_hud_reference_rect(id,candidate,&ctx,w,h);
            candidate.offset[0]+=(rect.x/grid).round()*grid-rect.x;
            candidate.offset[1]+=(rect.y/grid).round()*grid-rect.y;
            *self.render_studio.hud_layout.element_mut(id)=candidate;
        }
    }

    /// A viewport-space proxy for the *same* geometry later transformed by
    /// `UiSnapshot.hud_layout` in the Studio render target. The native render
    /// surface can be larger than this Program Preview rectangle.
    fn egui_studio_hud_overlay(&mut self,ui:&mut egui::Ui,viewport:egui::Rect,locked:bool) {
        if !self.render_studio.hud_editing || locked {return;}
        use crate::ui::{HudElementId,HudRectContext};
        let (w,h)=self.window.as_ref().map(|window|{
            let s=window.inner_size();(s.width.max(1),s.height.max(1))
        }).unwrap_or((1280,720));
        let virtual_layout=self.render_studio.hud_layout;
        let actual_layout=virtual_layout.studio_for_frame(w,h);
        let video=crate::ui::VideoSettings {draw_fps:0,draw_timer:false,..self.video};
        let score_mode=self.render_studio.sources.first()
            .map_or(2,|source|if source.playback.client_game.gametype()==3 {3}else{2});
        let ctx=HudRectContext::new(self.movement_keys_hud,&video,&self.perf,
            self.threads.len(),&self.japro_cg,self.strafe_helper,false,
            [0.,0.],score_mode);
        let defaults=self.render_studio.hud_settings.clone();
        let effective=self.render_studio.timeline.demo_at(self.render_studio.timeline_time())
            .map(|clip|defaults.effective(&clip.hud_overrides)).unwrap_or(defaults);
        if virtual_layout.snap_to_grid {
            let grid=virtual_layout.grid_size.clamp(1.,64.);
            let color=egui::Color32::from_rgba_premultiplied(20,24,19,28);
            let step_x=grid/1280.*viewport.width();
            let step_y=grid/720.*viewport.height();
            let mut x=viewport.left();
            while x<viewport.right() {
                ui.painter().line_segment([egui::pos2(x,viewport.top()),egui::pos2(x,viewport.bottom())],
                    egui::Stroke::new(0.5,color));x+=step_x;
            }
            let mut y=viewport.top();
            while y<viewport.bottom() {
                ui.painter().line_segment([egui::pos2(viewport.left(),y),egui::pos2(viewport.right(),y)],
                    egui::Stroke::new(0.5,color));y+=step_y;
            }
        }
        // Larger hit rectangles register first; the small readout on top is
        // still selectable where HUD elements overlap.
        let mut elements=STUDIO_HUD_ITEMS.to_vec();
        elements.sort_by(|(a,_),(b,_)| {
            let area=|id:HudElementId| {
                let r=crate::ui::hud_element_rect(id,actual_layout.element(id),&ctx,w,h);
                if id==HudElementId::CrosshairName {44.0*44.0}else{r.width*r.height}
            };
            area(*b).total_cmp(&area(*a))
        });
        for (id,key) in elements {
            if !effective.enabled(key) {continue;}
            let mut native=crate::ui::hud_element_rect(id,actual_layout.element(id),&ctx,w,h);
            if id==HudElementId::CrosshairName {
                let layout=actual_layout.element(id);
                native.x=w as f32*0.5+layout.offset[0]-22.0*layout.scale;
                native.y=h as f32*0.5+layout.offset[1]-22.0*layout.scale;
                native.width=44.0*layout.scale;
                native.height=44.0*layout.scale;
            }
            let rect=egui::Rect::from_min_size(
                egui::pos2(viewport.left()+native.x/w as f32*viewport.width(),
                    viewport.top()+native.y/h as f32*viewport.height()),
                egui::vec2((native.width/w as f32*viewport.width()).max(1.),
                    (native.height/h as f32*viewport.height()).max(1.)));
            let rect=rect.intersect(viewport);
            if rect.width()<=0.0 || rect.height()<=0.0 {continue;}
            let selected=self.render_studio.hud_selected==Some(id);
            // Exclude the selected item's scale grip from the move target.
            // Otherwise egui may claim the press for moving before the resize
            // control gets a chance to own it.
            let move_rect=if selected && rect.width()>18. && rect.height()>18. {
                egui::Rect::from_min_max(rect.min,rect.max-egui::vec2(12.,12.))
            }else{rect};
            let response=ui.interact(move_rect,ui.make_persistent_id(("studio_hud_move",id)),
                egui::Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::Grab);
            if response.clicked()||response.drag_started(){
                self.render_studio.hud_selected=Some(id);
                self.render_studio.hud_drag_origin=Some(virtual_layout.element(id).offset);
            }
            if response.dragged() {
                let start=self.render_studio.hud_drag_origin.unwrap_or(virtual_layout.element(id).offset);
                let delta=response.total_drag_delta().unwrap_or(egui::Vec2::ZERO);
                let mut offset=[
                    (start[0]+delta.x*1280./viewport.width()).clamp(-16384.,16384.),
                    (start[1]+delta.y*720./viewport.height()).clamp(-16384.,16384.)
                ];
                if virtual_layout.snap_to_grid&&!ui.input(|input|input.modifiers.alt){
                    let step=virtual_layout.grid_size.clamp(1.,64.);
                    let candidate=crate::ui::HudElementLayout {offset,..virtual_layout.element(id)};
                    offset=studio_snap_position(id,candidate,&virtual_layout,&ctx,w,h,step,
                        |key|effective.enabled(key)).offset;
                }
                self.render_studio.hud_layout.element_mut(id).offset=offset;
                ui.ctx().request_repaint();
            }
            if response.drag_stopped(){self.render_studio.hud_drag_origin=None;}
            let color=if selected {theme::ACCENT}else{theme::TEXT_DIM};
            ui.painter().rect_stroke(rect,3.,egui::Stroke::new(if selected {2.} else {1.},color),
                egui::StrokeKind::Inside);
            let label=if id==HudElementId::CrosshairName {"CROSSHAIR"}
                else if id==HudElementId::Fps {"GAME TIMER"}else{id.label()};
            let tag=ui.painter().layout_no_wrap(label.to_owned(),egui::FontId::proportional(10.),color);
            let tag_pos=egui::pos2(rect.left()+3.,(rect.top()+3.).min(viewport.bottom()-12.));
            ui.painter().rect_filled(egui::Rect::from_min_size(tag_pos,tag.size()+egui::vec2(6.,2.)),2.,theme::SURFACE);
            ui.painter().galley(tag_pos+egui::vec2(3.,1.),tag,color);
            if selected {
                // Bottom-right resize grip scales the actual native HUD. The
                // entire layout is still saved as its normal HudElementLayout.
                let size=10.;
                let grip=egui::Rect::from_center_size(rect.right_bottom(),egui::vec2(size,size));
                let drag=ui.interact(grip,ui.make_persistent_id(("studio_hud_scale",id)),
                    egui::Sense::drag()).on_hover_cursor(egui::CursorIcon::ResizeNwSe);
                ui.painter().rect_filled(grip,1.,color);
                if drag.drag_started(){
                    self.render_studio.hud_scale_origin=Some(virtual_layout.element(id).scale);
                    self.render_studio.hud_drag_origin=None;
                }
                if drag.dragged(){
                    let start=self.render_studio.hud_scale_origin
                        .unwrap_or(virtual_layout.element(id).scale);
                    let delta=drag.total_drag_delta().unwrap_or(egui::Vec2::ZERO);
                    // Use the mouse-down scale to compute the denominator. A
                    // live denominator depends on the result of the previous
                    // frame, which causes the preview to jump during a drag.
                    let mut beginning=actual_layout.element(id);
                    beginning.scale=start;
                    let base=crate::ui::hud_element_rect(id,beginning,&ctx,w,h);
                    let base_w=if id==HudElementId::CrosshairName {44.0*start}else{base.width};
                    let base_h=if id==HudElementId::CrosshairName {44.0*start}else{base.height};
                    let denominator=(base_w/w as f32*viewport.width())
                        .max(base_h/h as f32*viewport.height()).max(12.);
                    let mut next=(start+(delta.x+delta.y)/denominator).clamp(0.5,2.);
                    if virtual_layout.snap_to_grid && !ui.input(|i|i.modifiers.alt) {
                        let step=virtual_layout.grid_size.clamp(1.,64.);
                        let mut unit=virtual_layout.element(id);
                        unit.scale=1.0;
                        let base=studio_hud_reference_rect(id,unit,&ctx,w,h);
                        next=crate::ui::snap_hud_dimensions_scale([base.width,base.height],next,step);
                        // Compare actual sizes in preview points. A candidate
                        // must be spatially near, not just an unrelated panel
                        // with the same pixel dimensions on the other side.
                        let mut trial=actual_layout.element(id);
                        trial.scale=next;
                        let mut proposed=crate::ui::hud_element_rect(id,trial,&ctx,w,h);
                        if id==HudElementId::CrosshairName {
                            proposed.x=w as f32*0.5+trial.offset[0]-22.0*next;
                            proposed.y=h as f32*0.5+trial.offset[1]-22.0*next;
                            proposed.width=44.0*next;proposed.height=44.0*next;
                        }
                        let proposed=crate::ui::HudRect {
                            x:viewport.left()+proposed.x/w as f32*viewport.width(),
                            y:viewport.top()+proposed.y/h as f32*viewport.height(),
                            width:if id==HudElementId::CrosshairName {44.0*next/w as f32*viewport.width()} else {proposed.width/w as f32*viewport.width()},
                            height:if id==HudElementId::CrosshairName {44.0*next/h as f32*viewport.height()} else {proposed.height/h as f32*viewport.height()},
                        };
                        let neighbors=STUDIO_HUD_ITEMS.iter().copied()
                            .filter(|(other,key)|*other!=id && effective.enabled(key))
                            .map(|(other,_)|{
                                let mut r=crate::ui::hud_element_rect(other,
                                    actual_layout.element(other),&ctx,w,h);
                                if other==HudElementId::CrosshairName {
                                    let l=actual_layout.element(other);
                                    r.x=w as f32*0.5+l.offset[0]-22.0*l.scale;
                                    r.y=h as f32*0.5+l.offset[1]-22.0*l.scale;
                                    r.width=44.0*l.scale;r.height=44.0*l.scale;
                                }
                                crate::ui::HudRect {
                                    x:viewport.left()+r.x/w as f32*viewport.width(),
                                    y:viewport.top()+r.y/h as f32*viewport.height(),
                                    width:r.width/w as f32*viewport.width(),
                                    height:r.height/h as f32*viewport.height(),
                                }
                            });
                        let (matching,matched)=crate::ui::snap_hud_neighbor_size(
                            proposed,neighbors,12.0,32.0);
                        if matched[0] || matched[1] {
                            // Scale is uniform for these native elements.
                            // Prefer the axis needing the smallest correction.
                            let x_diff=(matching[0]-proposed.width).abs();
                            let y_diff=(matching[1]-proposed.height).abs();
                            next=if matched[0] && (!matched[1] || x_diff<=y_diff) {
                                (next*matching[0]/proposed.width.max(1.0)).clamp(0.5,2.0)
                            }else{
                                (next*matching[1]/proposed.height.max(1.0)).clamp(0.5,2.0)
                            };
                            ui.painter().text(rect.right_top()+egui::vec2(4.,0.),
                                egui::Align2::LEFT_TOP,
                                if matched[0] && (!matched[1] || x_diff<=y_diff) {"MATCH WIDTH"}
                                else {"MATCH HEIGHT"},
                                egui::FontId::proportional(10.),theme::ACCENT);
                        }
                    }
                    let mut candidate=virtual_layout.element(id);
                    candidate.scale=next;
                    if virtual_layout.snap_to_grid && !ui.input(|i|i.modifiers.alt) {
                        candidate=studio_snap_position(id,candidate,&virtual_layout,&ctx,w,h,
                            virtual_layout.grid_size.clamp(1.,64.),|key|effective.enabled(key));
                    }
                    *self.render_studio.hud_layout.element_mut(id)=candidate;
                    ui.ctx().request_repaint();
                }
                if drag.drag_stopped(){self.render_studio.hud_scale_origin=None;}
            }
        }
        ui.painter().text(egui::pos2(viewport.left()+8.,viewport.bottom()-8.),
            egui::Align2::LEFT_BOTTOM,"HUD EDIT · drag to grid/neighbor edges · corner scales to grid/nearby size · Alt bypasses",
            egui::FontId::proportional(11.),theme::ACCENT);
    }

    fn egui_studio_hud_inspector(&mut self,ui:&mut egui::Ui) {
        use crate::ui::{HudAnchor,HudLayout};
        sui::inspector_title(ui,theme::ACCENT,"Studio HUD","Export layout",
            "Drag on the Program Preview. This layout is saved to the selected Studio CFG, never to gameplay.");
        ui.horizontal_wrapped(|ui| {
            if ui.small_button("Copy gameplay").clicked() {
                let (w,h)=self.window.as_ref().map(|window|{
                    let size=window.inner_size();(size.width,size.height)
                }).unwrap_or((1280,720));
                let mut copied=self.hud_layout.studio_from_frame(w,h);
                // CrosshairName becomes the crosshair itself only inside Studio.
                copied.crosshair_name=HudLayout::default().crosshair_name;
                self.render_studio.hud_layout=copied;
                self.align_studio_defaults_for_editor();
            }
            if ui.small_button("Reset all").clicked(){
                self.render_studio.hud_layout=HudLayout::default();
                self.align_studio_defaults_for_editor();
            }
            if ui.small_button("Done").clicked(){self.render_studio.hud_editing=false;}
        });
        ui.separator();
        let flags=self.render_studio.hud_settings.clone();
        let mut selected=self.render_studio.hud_selected;
        let display_name=|id:crate::ui::HudElementId|match id {
            crate::ui::HudElementId::CrosshairName=>"CROSSHAIR",
            crate::ui::HudElementId::Fps=>"GAME TIMER",
            _=>id.label(),
        };
        egui::ComboBox::from_id_salt("studio_hud_selected_element")
            .selected_text(selected.map_or("Select an element",display_name))
            .width(ui.available_width())
            .show_ui(ui,|ui|{for &(id,_) in STUDIO_HUD_ITEMS {ui.selectable_value(&mut selected,Some(id),display_name(id));}});
        if selected!=self.render_studio.hud_selected {self.render_studio.hud_selected=selected;}
        if let Some(id)=selected {
            let (_,key)=STUDIO_HUD_ITEMS.iter().find(|(item,_)|*item==id).copied().unwrap();
            let mut enabled=flags.enabled(key);
            if ui.checkbox(&mut enabled,"Show in Studio").changed(){self.render_studio.hud_settings.set(key,enabled);}
            let mut layout=self.render_studio.hud_layout.element(id);
            ui.horizontal(|ui| {
                ui.label("X");ui.add(egui::DragValue::new(&mut layout.offset[0]).speed(1.).range(-16384.0..=16384.0));
                ui.label("Y");ui.add(egui::DragValue::new(&mut layout.offset[1]).speed(1.).range(-16384.0..=16384.0));
            });
            ui.horizontal(|ui| {
                ui.label("Scale");ui.add(egui::Slider::new(&mut layout.scale,0.5..=2.).show_value(true));
            });
            if id.is_anchored() {
                const ANCHORS:&[(HudAnchor,&str)]=&[
                    (HudAnchor::TopLeft,"Top left"),(HudAnchor::TopCenter,"Top center"),(HudAnchor::TopRight,"Top right"),
                    (HudAnchor::CenterLeft,"Center left"),(HudAnchor::Center,"Center"),(HudAnchor::CenterRight,"Center right"),
                    (HudAnchor::BottomLeft,"Bottom left"),(HudAnchor::BottomCenter,"Bottom center"),(HudAnchor::BottomRight,"Bottom right")];
                egui::ComboBox::from_id_salt("studio_hud_anchor").selected_text(layout.anchor.short_name())
                    .show_ui(ui,|ui|{for &(anchor,label) in ANCHORS {ui.selectable_value(&mut layout.anchor,anchor,label);}});
            }
            if layout.scale != self.render_studio.hud_layout.element(id).scale
                && self.render_studio.hud_layout.snap_to_grid && !ui.input(|i|i.modifiers.alt) {
                let (w,h)=self.window.as_ref().map(|window|{
                    let size=window.inner_size();(size.width.max(1),size.height.max(1))
                }).unwrap_or((1280,720));
                let video=crate::ui::VideoSettings{draw_fps:0,draw_timer:false,..self.video};
                let ctx=crate::ui::HudRectContext::new(self.movement_keys_hud,&video,&self.perf,
                    self.threads.len(),&self.japro_cg,self.strafe_helper,false,[0.,0.],2);
                let mut unit=layout;unit.scale=1.;
                let base=studio_hud_reference_rect(id,unit,&ctx,w,h);
                layout.scale=crate::ui::snap_hud_dimensions_scale([base.width,base.height],
                    layout.scale,self.render_studio.hud_layout.grid_size.clamp(1.,64.));
                layout=studio_snap_position(id,layout,&self.render_studio.hud_layout,&ctx,w,h,
                    self.render_studio.hud_layout.grid_size.clamp(1.,64.),|key|flags.enabled(key));
            }
            *self.render_studio.hud_layout.element_mut(id)=layout;
            if ui.small_button("Reset selected").clicked(){
                self.render_studio.hud_layout.reset_element(id);
                self.align_studio_defaults_for_editor();
            }
        }
        ui.separator();
        ui.checkbox(&mut self.render_studio.hud_layout.snap_to_grid,"Snap to grid");
        if self.render_studio.hud_layout.snap_to_grid {
            ui.add(egui::Slider::new(&mut self.render_studio.hud_layout.grid_size,1.0..=64.0).text("Grid size"));
        }
        ui.label(egui::RichText::new("Reference: 1280×720. Move snaps visible edges to grid and adjacent panels; scaling uses grid-sized widths/heights and nearby matches. Alt bypasses snapping. Studio CFGs are separate.").size(11.).color(theme::TEXT_DIM));
    }

    pub(super) fn studio_locked(&self)->bool {
        self.render_studio.export.is_some() || self.render_studio.still.is_some() || self.render_studio.dm26_capture.is_some()
    }

    /// Project-level controls in the global top bar: name, history, files.
    pub(in crate::app::egui_menu) fn egui_render_studio_header_controls(&mut self, ui: &mut egui::Ui) {
        let locked=self.studio_locked();
        ui.scope(|ui| {
            sui::editor_style(ui);
            ui.spacing_mut().item_spacing.x=2.0;
            let (bar,_)=ui.allocate_exact_size(egui::vec2(1.0,18.0),egui::Sense::hover());
            ui.painter().rect_filled(bar,0.0,theme::LINE);
            ui.add_space(12.0);
            let name=if self.render_studio.project_name.trim().is_empty() {"Untitled project"}
                else {self.render_studio.project_name.as_str()};
            ui.label(egui::RichText::new(name).size(13.0).color(theme::TEXT))
                .on_hover_text(format!("Project file: {}",self.studio_project_path().display()));
            ui.add_space(14.0);
            if sui::icon_button(ui,Icon::Undo,!locked,false,"Undo (Ctrl+Z)").clicked()
                && self.render_studio.undo_edit() {
                self.profile_preview_key=None;
                self.refresh_studio_composite_anchors_after_history();
            }
            if sui::icon_button(ui,Icon::Redo,!locked,false,"Redo (Ctrl+Y)").clicked()
                && self.render_studio.redo_edit() {
                self.profile_preview_key=None;
                self.refresh_studio_composite_anchors_after_history();
            }
            ui.add_space(8.0);
            if sui::button(ui,Some(Icon::Plus),"New",ButtonKind::Normal,!locked)
                .on_hover_text("New project (Ctrl+N)").clicked() {
                self.request_new_render_studio_project();
            }
            if sui::icon_button(ui,Icon::Open,!locked,false,"Open project…").clicked() {
                self.render_studio.show_open_project=true;
            }
            if sui::icon_button(ui,Icon::Save,!locked,false,"Save project").clicked() {
                self.save_render_studio_project();
            }
            let more=sui::icon_button(ui,Icon::More,!locked,false,"More project actions");
            egui::Popup::menu(&more).style(sui::menu_style()).show(|ui| {
                sui::editor_style(ui);
                ui.set_min_width(240.0);
                if ui.button("New project…").clicked() {self.request_new_render_studio_project();ui.close();}
                if ui.button("Save As…").clicked() {self.render_studio.show_save_as=true;ui.close();}
                if ui.button("Open…").clicked() {self.render_studio.show_open_project=true;ui.close();}
                ui.separator();
                let mut live=self.render_studio.live_gpu_transitions;
                if ui.checkbox(&mut live,"Live GPU demo transition preview").changed() {
                    self.render_studio.live_gpu_transitions=live;
                    self.render_studio.preview_sample_time=None;
                }
            });
        });
    }

    /// Right side of the top bar, laid out right-to-left before BACK.
    pub(in crate::app::egui_menu) fn egui_render_studio_header_actions(&mut self, ui: &mut egui::Ui) {
        let locked=self.studio_locked();
        ui.scope(|ui| {
            sui::editor_style(ui);
            if sui::button(ui,Some(Icon::Export),"Export",ButtonKind::Primary,!locked)
                .on_hover_text("Video, depth and still-image export").clicked() {
                self.render_studio.cinematic_preview=None;
                self.profile_preview_key=None;
                self.render_studio.show_render=true;
            }
        });
    }

    fn studio_pixel_viewport(rect:egui::Rect,scale:f32)->[u32;4] {
        let scale=scale.max(0.01);
        [ (rect.min.x*scale).round().max(0.) as u32,
          (rect.min.y*scale).round().max(0.) as u32,
          (rect.width()*scale).round().max(1.) as u32,
          (rect.height()*scale).round().max(1.) as u32 ]
    }

    pub(in crate::app::egui_menu) fn egui_render_studio_page(&mut self, ui: &mut egui::Ui) {
        self.ensure_profile_catalog();
        self.studio_poll_route_job(ui.ctx());
        self.studio_poll_director_job(ui.ctx());
        if !self.render_studio.library_rows_next.is_empty() {
            self.render_studio.library_rows=std::mem::take(&mut self.render_studio.library_rows_next);
        }
        self.render_studio.library_hovered=false;
        // Metadata indexing runs on the existing demo-browser worker. Complete
        // any pending library drop only after its real duration is known.
        if self.render_studio.media_tab==StudioMediaTab::Demos ||
            self.render_studio.pending_demo_drop.is_some() || !self.render_studio.pending_demo_queue.is_empty() {
            self.poll_demo_metadata_jobs();
            if self.render_studio.pending_demo_drop.is_none() {
                if let Some((name,game,layer))=self.render_studio.pending_demo_queue.pop_front() {
                    let at=self.render_studio.timeline.demos.iter().filter(|c|c.layer==layer)
                        .map(|c|c.end()).fold(0.0,f64::max);
                    self.render_studio_queue_demo_drop(name,game,at,layer);
                }
            }
            if let Some((name,game,time,layer))=self.render_studio.pending_demo_drop.clone() {
                if let Some(metadata)=self.demo_metadata_cache.get(&name).cloned() {
                    self.render_studio.pending_demo_drop=None;
                    self.render_studio_place_demo(name,game,time,metadata.duration_ms,layer,None);
                } else if let Some(error)=self.demo_metadata_errors.get(&name).cloned() {
                    self.render_studio.pending_demo_drop=None;
                    self.render_studio.status=format!("Demo indexing failed: {error}");
                }
            }
        }
        if self.profile_background_maps.is_none() {
            self.profile_background_maps = Some(Vec::new());
            self.ui_catalog.request(&self.base, self.game.as_deref(),
                crate::app::ui_catalog::CatalogRequest::ProfileBackgroundMaps);
        }
        let width = ui.available_width();
        let height = ui.available_height().max(300.0);
        let (root, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let Some(panels) = render_studio_layout::compute(
            width, height, self.render_studio.timeline.track_count(), &self.render_studio.panels) else { return };
        let rect = |b: render_studio_layout::Bounds| bounds_rect(root.min, b);
        let has_inspector = panels.inspector[2] > panels.inspector[0];
        let viewport = rect(panels.viewport);
        // A fixed export resolution letterboxes the program viewer to its
        // aspect, so the preview frames exactly what the video will contain.
        let viewport = match self.render_studio.offscreen_output_size() {
            Some([w, h]) if viewport.height() > 1.0 => {
                let aspect = w as f32 / h.max(1) as f32;
                let fit = if viewport.width() / viewport.height() > aspect {
                    egui::vec2(viewport.height() * aspect, viewport.height())
                } else {
                    egui::vec2(viewport.width(), viewport.width() / aspect)
                };
                egui::Rect::from_center_size(viewport.center(), fit)
            }
            _ => viewport,
        };
        // Editor background around the panes. The viewport itself stays
        // unpainted: the renderer composites the 3D view into exactly it.
        {
            let p=ui.painter();
            for band in [
                egui::Rect::from_min_max(root.min,egui::pos2(root.right(),viewport.top())),
                egui::Rect::from_min_max(egui::pos2(root.left(),viewport.bottom()),root.max),
                egui::Rect::from_min_max(egui::pos2(root.left(),viewport.top()),egui::pos2(viewport.left(),viewport.bottom())),
                egui::Rect::from_min_max(egui::pos2(viewport.right(),viewport.top()),egui::pos2(root.right(),viewport.bottom())),
            ] {p.rect_filled(band,0.0,sui::GUTTER);}
        }
        // The pane gutters double as resizing handles. The anchor is stored
        // on pointer-down, so resizing has no cumulative drag feedback.
        let separators = [
            (0u8,egui::Rect::from_min_max(
                egui::pos2(rect(panels.library).right(),root.top()),
                egui::pos2(rect(panels.preview).left(),rect(panels.preview).bottom())),
                rect(panels.library).width()),
            (1u8,egui::Rect::from_min_max(
                egui::pos2(rect(panels.preview).right(),root.top()),
                egui::pos2(rect(panels.inspector).left(),rect(panels.preview).bottom())),
                rect(panels.inspector).width()),
            (2u8,egui::Rect::from_min_max(
                egui::pos2(root.left(),rect(panels.preview).bottom()),
                egui::pos2(root.right(),rect(panels.timeline).top())),
                rect(panels.timeline).height()),
        ];
        for (kind,grip,initial) in separators {
            if grip.width()<0.0 || grip.height()<0.0 || (kind==1 && !has_inspector) {continue;}
            let region=grip.expand(if kind==2 {3.0}else{4.0});
            let r=ui.interact(region,ui.make_persistent_id(("studio_panel_grip",kind)),
                egui::Sense::click_and_drag());
            let cursor=if kind==2 {egui::CursorIcon::ResizeVertical}
                else {egui::CursorIcon::ResizeHorizontal};
            let active=r.hovered() || r.dragged();
            let r=r.on_hover_cursor(cursor);
            if active {
                let c=grip.center();
                let line=if kind==2 {
                    egui::Rect::from_center_size(c,egui::vec2(grip.width(),2.0))
                } else {egui::Rect::from_center_size(c,egui::vec2(2.0,grip.height()))};
                ui.painter().rect_filled(line,1.0,theme::ACCENT_DEEP);
            }
            if r.drag_started() {
                if let Some(pos)=r.interact_pointer_pos() {
                    self.render_studio.panels.resize_drag=Some((kind,
                        if kind==2 {pos.y}else{pos.x},initial));
                }
            }
            if r.dragged() {
                if let (Some((drag_kind,origin,size)),Some(pos))=(
                    self.render_studio.panels.resize_drag,r.interact_pointer_pos()) {
                    if drag_kind==kind {
                        let delta=if kind==2 {origin-pos.y}
                            else if kind==1 {origin-pos.x}else{pos.x-origin};
                        let wanted=size+delta;
                        match kind {
                            0=>{self.render_studio.panels.library_collapsed=false;
                                self.render_studio.panels.library_width=Some(wanted.clamp(120.0,(width*0.55).max(120.0)));}
                            1=>{self.render_studio.panels.inspector_collapsed=false;
                                self.render_studio.panels.inspector_width=Some(wanted.clamp(120.0,(width*0.5).max(120.0)));}
                            _=>self.render_studio.panels.timeline_height=Some(wanted.clamp(130.0,(height-170.0).max(130.0))),
                        }
                        ui.ctx().request_repaint();
                    }
                }
            }
            if r.drag_stopped(){self.render_studio.panels.resize_drag=None;}
        }
        let locked = self.studio_locked();
        // Clicking anywhere on the timeline returns keyboard ownership to
        // the NLE, including after searching the library or naming a project.
        // This is more precise than the whole-context keyboard-ownership flag.
        let timeline_hit=rect(panels.timeline);
        let scene_hit=viewport;
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
        if self.render_studio.timeline.selected_clip.is_some() || self.render_studio.timeline.selected_demo.is_some() ||
            self.render_studio.timeline.selected_camera.is_some() || self.render_studio.selected_background {
            self.render_studio.audio.selected=None;self.render_studio.audio.selected_track=None;self.render_studio.audio.selected_key=None;
        }
        self.render_studio.init_history();
        self.studio_sync_light_selection();
        if !locked && !self.render_studio.hud_editing && self.render_studio.spatial.drag.is_none() && self.render_studio.cinematic_preview.is_none() {
            self.studio_keyboard_shortcuts(ui.ctx());
        }

        // ---- Media browser ------------------------------------------------
        let library = rect(panels.library);
        if self.render_studio.panels.library_collapsed {
            if collapsed_rail(ui,library,"MEDIA",Icon::ChevronRight,"studio_library_rail") {
                self.render_studio.panels.library_collapsed=false;
            }
        } else {
            sui::paint_pane(ui.painter(),library);
            let tools=sui::pane_header(ui.painter(),rect(panels.library_header),"MEDIA");
            sui::region(ui,tools,"studio_library_tools",egui::Layout::right_to_left(egui::Align::Center),|ui| {
                if sui::icon_button(ui,Icon::ChevronLeft,true,false,"Collapse media browser").clicked() {
                    self.render_studio.panels.library_collapsed=true;
                }
            });
            let body=rect(panels.library_body).shrink2(egui::vec2(8.0,6.0));
            sui::region(ui,body,"studio_library",egui::Layout::top_down(egui::Align::Min),|ui| {
                self.egui_render_studio_media_library(ui,!has_inspector);
            });
            self.egui_render_studio_library_footer(ui,rect(panels.library_footer));
        }

        // ---- Program viewer -----------------------------------------------
        let pixel_viewport = Self::studio_pixel_viewport(viewport,ui.ctx().pixels_per_point());
        self.render_studio.demo_viewport = Some(pixel_viewport);
        let demo_footage = self.render_studio.timeline.demo_at(self.render_studio.timeline_time()).is_some();
        // The renderer composites the offscreen world only into this rectangle.
        // Never use opaque egui rectangles to conceal a fullscreen demo.
        // Keep the viewport latch armed for the whole editor lifetime, including
        // the first demo load and gaps between clips. Asset-only frames bypass
        // the world compositor via SetAssetPreviewMode; native demo frames never
        // get to fall back to a fullscreen swapchain draw.
        let compositor = Some(pixel_viewport);
        if self.render_studio_world_compositor != compositor {
            self.render_studio_world_compositor = compositor;
            self.render_command(crate::app::RenderCommand::SetStudioWorldViewport(compositor));
        }
        let demo_ready = demo_footage && self.studio_demo_scene_active();
        if demo_footage && !demo_ready {
            ui.painter().rect_filled(viewport,0.0,sui::WELL);
        }
        let header=rect(panels.preview_header);
        let tools=sui::pane_header(ui.painter(),header,
            if self.render_studio.cinematic_preview.is_some() {"SOURCE"} else {"PROGRAM"});
        // Background is a project setting in the viewer, rather than a track.
        let profile_w=330.0_f32.min(tools.width()*0.6);
        let split=tools.right()-profile_w;
        sui::region(ui,egui::Rect::from_min_max(tools.min,egui::pos2(split-6.0,tools.bottom())),
            "studio_viewer_tools",egui::Layout::left_to_right(egui::Align::Center),|ui| {
            self.egui_studio_viewer_tools(ui,viewport,locked,demo_footage);
        });
        sui::region(ui,egui::Rect::from_min_max(egui::pos2(split,tools.top()),
            egui::pos2(tools.right()-30.0,tools.bottom())),
            "studio_viewer_profile",egui::Layout::left_to_right(egui::Align::Center),|ui| {
            let background=sui::settings_button(ui,"studio_background_button","Background",!locked)
                .on_hover_text("Set the background behind character footage");
            let mut background_open=self.render_studio.background_popup_open;
            sui::settings_popup(&background,&mut background_open,|ui| {
                sui::editor_style(ui);
                ui.set_width(280.0);
                ui.add_enabled_ui(!locked,|ui| self.egui_studio_background_settings(ui));
                if demo_footage {sui::hint(ui,"Demo clips show their recorded world. This background applies to character footage.");}
            });
            self.render_studio.background_popup_open=background_open;
            let lighting=sui::settings_button(ui,"studio_lighting_button","Lighting",!locked)
                .on_hover_text("Ambient fill and directional sun for Render Studio character scenes");
            let mut lighting_open=self.render_studio.scene_lighting_popup_open;
            sui::settings_popup(&lighting,&mut lighting_open,|ui| {
                sui::editor_style(ui);
                ui.set_width(324.0);
                ui.add_enabled_ui(!locked,|ui|self.egui_studio_scene_lighting_settings(ui));
                if demo_footage {sui::hint(ui,"Recorded demo worlds keep their own BSP lighting.");}
            });
            self.render_studio.scene_lighting_popup_open=lighting_open;
            let w=(ui.available_width()-34.0).clamp(60.0,200.0);
            ui.add_space((ui.available_width()-w-34.0).max(0.0));
            self.egui_studio_profile_picker(ui,locked,w);
        });
        // Shortcuts are reference material, not project properties. Keep them
        // behind a persistent help control in the Program/Source viewer header.
        sui::region(ui,egui::Rect::from_min_max(egui::pos2(tools.right()-26.0,tools.top()),tools.max),
            "studio_viewer_shortcuts",egui::Layout::left_to_right(egui::Align::Center),|ui| {
            let help=sui::icon_button(ui,Icon::Help,true,false,"Render Studio shortcuts");
            egui::Popup::menu(&help).style(sui::menu_style()).show(|ui| {
                sui::editor_style(ui);
                ui.set_min_width(330.0);
                sui::group_label(ui,"Keyboard & timeline shortcuts");
                for (key,action) in [
                    ("Space","Play / pause"), ("Left / Right","Step one frame"),
                    ("K","Split at playhead"), ("I / O","Set export IN / OUT"),
                    ("Ctrl+C / V","Copy / paste selection"), ("Del","Delete selection"),
                    ("Ctrl+Z / Y","Undo / redo"), ("Alt+wheel","Zoom timeline at pointer"),
                    ("Ctrl / Shift+click","Toggle / extend selection"),
                ] {
                    sui::prop_value(ui,key,action);
                }
                ui.add_space(4.0);
                sui::hint(ui,"Each character track animates a separate actor; overlapping tracks render together.");
                ui.separator();
                sui::group_label(ui,"Viewer navigation");
                sui::prop_value(ui,"Right mouse","Look around");
                sui::prop_value(ui,"WASD","Move camera");
                sui::prop_value(ui,"E / C","Move up / down");
                sui::prop_value(ui,"Wheel",&format!("Fly speed ({:.0})",self.render_studio.navigation.speed));
                sui::hint(ui,"Drag a key marker to move it; select it for X / Y / Z handles.");
            });
        });
        let response = ui.interact(viewport,ui.make_persistent_id("studio_stage_navigation"),
            if self.render_studio.hud_editing {egui::Sense::hover()} else {egui::Sense::click_and_drag()});
        if !locked && !demo_footage {self.set_asset_preview_viewport(viewport,ui.ctx().pixels_per_point());}
        let light_busy=if self.render_studio.hud_editing {false}else{self.egui_studio_light_overlay(ui,viewport,locked)};
        let spatial_busy=if demo_footage || self.render_studio.hud_editing {false}else{self.egui_studio_spatial_overlay(ui,viewport,locked)};
        let camera_busy=if self.render_studio.hud_editing {false}else{self.egui_studio_camera_handles(ui,viewport,locked)};
        let overlay_busy=light_busy||spatial_busy||camera_busy;
        if !self.render_studio.hud_editing && (!demo_footage || self.render_studio.spatial.scene_camera.is_some()) {
            self.egui_studio_preview_navigation(ui,&response,overlay_busy,locked);
        }
        self.egui_studio_hud_overlay(ui,viewport,locked);
        self.egui_studio_viewport_hints(ui,viewport,demo_footage,demo_ready);
        ui.painter().rect_stroke(viewport,0.0,egui::Stroke::new(1.0,theme::HAIRLINE),egui::StrokeKind::Inside);
        self.egui_studio_transport(ui,rect(panels.transport),locked);

        // ---- Inspector ----------------------------------------------------
        if has_inspector {
            let inspector = rect(panels.inspector);
            if self.render_studio.panels.inspector_collapsed {
                if collapsed_rail(ui,inspector,"INSPECTOR",Icon::ChevronLeft,"studio_inspector_rail") {
                    self.render_studio.panels.inspector_collapsed=false;
                }
            } else {
                sui::paint_pane(ui.painter(),inspector);
                let tools=sui::pane_header(ui.painter(),rect(panels.inspector_header),"INSPECTOR");
                sui::region(ui,tools,"studio_inspector_tools",egui::Layout::right_to_left(egui::Align::Center),|ui| {
                    if sui::icon_button(ui,Icon::ChevronRight,true,false,"Collapse inspector").clicked() {
                        self.render_studio.panels.inspector_collapsed=true;
                    }
                });
                let body=rect(panels.inspector_body).shrink2(egui::vec2(2.0,4.0));
                sui::region(ui,body,"studio_inspector",egui::Layout::top_down(egui::Align::Min),|ui| {
                    egui::ScrollArea::vertical().id_salt("studio_inspector_scroll").auto_shrink([false,false])
                        .show(ui,|ui| {
                            egui::Frame::new().inner_margin(egui::Margin::symmetric(8,4)).show(ui,|ui| {
                                ui.set_width(ui.available_width());
                                self.egui_render_studio_inspector(ui);
                            });
                        });
                });
            }
        }

        // Console-style neon edge under the top bar.
        sui::neon_edge(ui.painter(),egui::Rect::from_min_size(root.min,egui::vec2(root.width(),2.0)));

        // ---- Timeline -----------------------------------------------------
        self.egui_render_studio_timeline(ui,rect(panels.timeline),rect(panels.timeline_header),
            rect(panels.ruler),rect(panels.tracks),rect(panels.scrollbar),locked);
        if ui.ctx().input(|i|i.pointer.any_released()) {self.render_studio.media_drag=None;self.render_studio.media_drag_group.clear();}
        self.egui_render_studio_project_dialogs(ui.ctx());
        self.egui_render_studio_file_picker(ui.ctx());
        self.egui_render_studio_model_picker(ui.ctx());
        self.egui_render_studio_render_dialog(ui.ctx());
        self.egui_render_studio_graphics_settings(ui.ctx());
        self.egui_render_studio_leave_dialog(ui.ctx());
        if self.render_studio.sources.iter().any(|s|!s.gpu_ready && !s.failed)
            || self.render_studio.bsp_world.as_ref().is_some_and(|w|!w.ready && w.error.is_none()) {ui.ctx().request_repaint();}
        if !demo_footage {self.update_profile_preview();}
        let (pointer_down,edit_input)=ui.ctx().input(|i|(i.pointer.primary_down(),
            !i.raw.dropped_files.is_empty() || i.events.iter().any(|e|!matches!(e,
                egui::Event::PointerMoved(_)|egui::Event::MouseMoved(_)|egui::Event::PointerGone|
                egui::Event::ModifiersChanged(_)|egui::Event::WindowFocused(_)|egui::Event::Screenshot{..}))));
        self.render_studio.track_ui_edit(pointer_down,edit_input);
        if !locked && (self.render_studio.timeline.playing || self.render_studio.cinematic_preview.as_ref().is_some_and(|p|p.playing)) {ui.ctx().request_repaint();}
    }

    fn egui_studio_background_settings(&mut self,ui:&mut egui::Ui) {
        let mut background=self.render_studio.background;
        studio_background_choices(ui,&mut background);
        if background!=self.render_studio.background {self.render_studio.background=background;
            self.render_studio_update_backdrop();}
        if background==StudioBackground::Bsp && self.render_studio.bsp_maps.is_none() {
            self.render_studio.bsp_maps=Some(Vec::new());
            self.ui_catalog.request(&self.base,self.game.as_deref(),
                crate::app::ui_catalog::CatalogRequest::StudioBspMaps);
        }
        if background==StudioBackground::Levelshot || background==StudioBackground::Bsp {
            let maps=if background==StudioBackground::Bsp {
                self.render_studio.bsp_maps.clone().unwrap_or_default()
            }else{self.profile_background_maps.clone().unwrap_or_default()};
            let mut chosen=self.render_studio.background_map.clone();
            let no_maps=maps.is_empty();
            sui::prop_row(ui,"Map",|ui| {
                sui::combo_box("studio_background_map_inspector").width(ui.available_width())
                    .height(320.0)
                    .selected_text(if chosen.is_empty(){if background==StudioBackground::Bsp {"Select BSP…".to_owned()}else{"Select levelshot…".to_owned()}}else{chosen.clone()})
                    .show_ui(ui,|ui|{for map in maps {ui.selectable_value(&mut chosen,map.clone(),map);}});
            });
            if chosen!=self.render_studio.background_map {
                self.render_studio.background_map=chosen;self.render_studio_update_backdrop();
            }
            if background==StudioBackground::Bsp {
                if let Some(world)=self.render_studio.bsp_world.as_ref() {
                    if let Some(error)=world.error.as_deref() {
                        sui::hint(ui,&format!("BSP failed: {error}"));
                    } else if !world.ready {
                        let p=world.progress.snapshot();
                        sui::hint(ui,&format!("{} · {} / {}",p.label,p.completed,p.total));
                    }else{sui::hint(ui,"Real BSP geometry, shared depth and moving Studio camera.");}
                    if world.error.is_some() && ui.button("Retry BSP load").clicked() {
                        if let Some(old)=self.render_studio.bsp_world.take() {
                            self.render_command(crate::app::RenderCommand::RemoveStudioSource(old.id));
                        }
                        self.render_studio_update_backdrop();
                    }
                }else if no_maps{sui::hint(ui,"No installed BSPs indexed yet.");}
            }
        }
    }

    /// Ctrl+A: everything of the kind in focus. Library rows when the pointer
    /// is over the library; else Director shots, demo clips, or animation
    /// clips + camera keys, following the current selection.
    fn studio_select_all(&mut self) {
        if self.render_studio.library_hovered {
            let rows=&self.render_studio.library_rows;
            self.render_studio.library_selection=rows.iter().map(|(key,_)|key.clone()).collect();
            self.render_studio.library_anchor=rows.first().map(|(key,_)|key.clone());
            return;
        }
        let timeline=&self.render_studio.timeline;
        let anim_items=!timeline.clips.is_empty() || timeline.camera.len()>1;
        if self.studio_director_index().is_some() {
            self.studio_select_all_directors();
        } else if timeline.selected_demo.is_some() || (!anim_items && !timeline.demos.is_empty()) {
            self.render_studio.timeline.select_all_demos();
            self.render_studio.selected_background=false;
        } else if !timeline.selected_items().is_empty() || anim_items {
            self.render_studio.timeline.select_all_items();
            self.render_studio.selected_background=false;
        } else if !timeline.director_shots.is_empty() {
            self.studio_select_all_directors();
        }
    }

    /// NLE keyboard shortcuts. Unlike the global menu's keyboard ownership
    /// bit, this checks for a focused edit widget, so shortcuts still work
    /// after clicking a timeline item even if the page owns keyboard input.
    fn studio_keyboard_shortcuts(&mut self, ctx:&egui::Context) {
        let typing = ctx.text_edit_focused();
        let (play,left,right,split,mark_in,mark_out) = ctx.input(|i| (
            i.key_pressed(egui::Key::Space),i.key_pressed(egui::Key::ArrowLeft),
            i.key_pressed(egui::Key::ArrowRight),i.key_pressed(egui::Key::K),
            i.key_pressed(egui::Key::I)||i.key_pressed(egui::Key::OpenBracket),
            i.key_pressed(egui::Key::O)||i.key_pressed(egui::Key::CloseBracket),
        ));
        let (ctrl,shift,undo,redo,delete)=ctx.input(|i|(
            i.modifiers.command || i.modifiers.ctrl,i.modifiers.shift,
            i.key_pressed(egui::Key::Z),i.key_pressed(egui::Key::Y),
            i.key_pressed(egui::Key::Delete)||i.key_pressed(egui::Key::Backspace)));
        let (select_all,regenerate)=ctx.input(|i|(i.key_pressed(egui::Key::A),i.key_pressed(egui::Key::R)));
        if !typing && ctrl && ctx.input(|i|i.key_pressed(egui::Key::N)) && !self.studio_locked() {
            self.request_new_render_studio_project();
        }
        if !typing && ctrl && select_all {self.studio_select_all();}
        if !typing && !ctrl && regenerate && self.studio_director_index().is_some() {self.studio_regenerate_selected();}
        if !typing && ctrl && undo && !shift {
            if self.render_studio.undo_edit(){
                self.profile_preview_key=None;self.render_studio_update_backdrop();
                self.refresh_studio_composite_anchors_after_history();
            }
        } else if !typing && ctrl && (redo || (shift && undo)) {
            if self.render_studio.redo_edit(){
                self.profile_preview_key=None;self.render_studio_update_backdrop();
                self.refresh_studio_composite_anchors_after_history();
            }
        } else if !typing && !ctrl && delete && self.render_studio.light_ui.selection.is_some() {
            self.studio_delete_light_selection();
        } else if !typing && !ctrl && delete && self.studio_director_index().is_some() {
            let ids=self.studio_director_selected_ids();
            self.studio_delete_directors(&ids);
        } else if !typing && !ctrl && delete {
            if let Some((track,i))=self.render_studio.audio.selected.take() {
                if let Some(clip)=self.render_studio.timeline.audio_tracks.get_mut(track).and_then(|t|t.clips.get_mut(i)) {
                    if let Some(k)=self.render_studio.audio.selected_key.take() {
                        if k<clip.keys.len() {clip.keys.remove(k);}
                        self.render_studio.audio.selected=Some((track,i));
                    } else {self.render_studio.timeline.audio_tracks[track].clips.remove(i);}
                }
            } else if self.render_studio.timeline.selected_speed.is_some() {
                if self.render_studio.timeline.delete_selected_speed_key() {
                    self.profile_preview_key=None;
                } else {self.render_studio.status="Removing the speed key would overlap another clip".into();}
            } else if self.render_studio.timeline.selected_demo.is_some() {
                let targets=self.render_studio.timeline.selected_demo_indices();
                self.render_studio.timeline.remove_demos(targets);
                self.profile_preview_key=None;
            } else if self.render_studio.timeline.delete_selection(){self.profile_preview_key=None;}
        }
        if self.render_studio.handle_clipboard_input(ctx) {
            self.profile_preview_key=None;
        }
        if !typing && play { self.render_studio.timeline.toggle_play(); }
        if !typing && split {
            let t=self.render_studio.timeline_time();
            if let Some((track,i))=self.render_studio.audio.selected {
                let pair=self.render_studio.timeline.audio_tracks.get(track)
                    .and_then(|row|row.clips.get(i)).and_then(|clip|clip.split_at(t));
                if let Some((left,right))=pair {
                    let row=&mut self.render_studio.timeline.audio_tracks[track];
                    row.clips[i]=left;row.clips.insert(i+1,right);
                    self.render_studio.audio.selected=Some((track,i+1));
                }
            } else {
                let i=self.render_studio.timeline.selected_clip.filter(|&i|
                    self.render_studio.timeline.clips.get(i).is_some_and(|c|t>c.start&&t<c.end()))
                    .or_else(||self.render_studio.timeline.clips.iter().position(|c|t>c.start&&t<c.end()));
                if let Some(i)=i {
                    if self.render_studio.timeline.split_clip(i,t){self.profile_preview_key=None;}
                }
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

    /// State messages drawn over the 3D view. Editor model-stage and native
    /// demo-scene hints are deliberately separate: the latter never guesses
    /// an actor pose from demo bytes.
    fn egui_studio_viewport_hints(&mut self,ui:&mut egui::Ui,viewport:egui::Rect,demo_footage:bool,demo_ready:bool) {
        let t = self.render_studio.timeline_time();
        let painter=ui.painter().with_clip_rect(viewport);
        if demo_footage {
            if demo_ready {return;}
            if self.render_studio.demo_preview.as_ref().is_some_and(|p| p.failed) {
                sui::empty_state(&painter,viewport,Icon::Demo,"Demo could not load",
                    "Check the Studio status line and the console for the reason.");
            } else {
                let progress = self.studio_loading_progress();
                let heading = if progress.label.starts_with("Seeking") { "Seeking recorded POV…" } else { "Loading recorded POV…" };
                let fraction = if progress.total == 0 { 0.25 } else {
                    (progress.completed as f64 / progress.total as f64).clamp(0.0, 1.0) as f32
                };
                let text = if progress.total == 0 { progress.label } else {
                    format!("{} · {} / {}", progress.label, progress.completed.min(progress.total), progress.total)
                };
                sui::loading_state(ui,viewport,Icon::Demo,heading,
                    "Loaded recordings stay cached while Keep demos loaded is enabled.",
                    fraction,&text,progress.total == 0);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
            }
            return;
        }
        if self.render_studio.cinematic_preview.is_some() {return;}
        let active = self.render_studio.timeline.active_actors(t);
        if active.is_empty() {
            sui::empty_state(&painter,viewport,Icon::Anim,"No character clip at the playhead",
                "Drag an animation from Media onto the timeline, or move the playhead onto a clip.");
            return;
        }
        if self.profile_dynamic_models.is_empty() {
            sui::overlay_pill(&painter,viewport.center_bottom()+egui::vec2(0.0,-14.0),egui::Align2::CENTER_BOTTOM,
                "No character surfaces produced for this model / animation: see console",theme::WARNING,12.0);
            return;
        }
        let first = active[0];
        let motion = first.motion_at(t, self.render_studio.timeline.interpolation !=
            crate::app::render_studio_timeline::StudioInterpolation::Linear);
        let center = glam::Vec3::new(
            88.0*self.profile_preview_zoom.clamp(0.45,2.5)+motion.position[0],
            18.0+motion.position[1],
            -22.0+first.lane as f32*26.0+motion.position[2]);
        let camera=self.asset_preview_camera();
        let direction=(center-camera.position).normalize_or_zero();
        if self.render_studio.spatial.scene_camera.is_none() && camera.forward().dot(direction) < 0.8 {
            sui::overlay_pill(&painter,viewport.center_bottom()+egui::vec2(0.0,-14.0),egui::Align2::CENTER_BOTTOM,
                "Camera is pointed away from the character: use Frame actor in the transport bar",theme::WARNING,12.0);
        }
    }

    /// Program transport: timecode, playback controls, shot tools.
    fn egui_studio_transport(&mut self,ui:&mut egui::Ui,rect:egui::Rect,locked:bool) {
        {
            let p=ui.painter();
            p.rect_filled(rect,egui::CornerRadius{nw:0,ne:0,sw:2,se:2},sui::PANE_HEADER);
            p.line_segment([rect.left_top(),rect.right_top()],egui::Stroke::new(1.0,theme::HAIRLINE));
        }
        if self.render_studio.cinematic_preview.is_some() {
            self.egui_studio_cinematic_transport(ui,rect);
            return;
        }
        let fps=self.render_studio.fps;
        let t=self.render_studio.timeline_time();
        let duration=self.render_studio.timeline.duration();
        let row=rect.shrink2(egui::vec2(sui::PAD,0.0));
        sui::region(ui,row,"studio_transport_left",egui::Layout::left_to_right(egui::Align::Center),|ui| {
            ui.label(egui::RichText::new(sui::timecode(t,fps)).monospace().size(16.0).color(theme::ACCENT))
                .on_hover_text(format!("Playhead {t:.3} s · timecode at the {fps} fps export rate"));
        });
        let center=egui::Rect::from_center_size(rect.center(),egui::vec2(5.0*26.0+4.0*4.0,26.0));
        sui::region(ui,center,"studio_transport_center",egui::Layout::left_to_right(egui::Align::Center),|ui| {
            ui.spacing_mut().item_spacing.x=4.0;
            let enabled=!locked;
            let step=1.0/f64::from(fps.max(1));
            if sui::icon_button(ui,Icon::ToStart,enabled,false,"Go to IN point").clicked() {
                let (begin,_)=self.render_studio.timeline.export_range();
                self.render_studio.timeline.seek(begin);self.profile_preview_key=None;
            }
            if sui::icon_button(ui,Icon::StepBack,enabled,false,"Previous frame (Left arrow)").clicked() {
                self.render_studio.timeline.seek(t-step);self.profile_preview_key=None;
            }
            let playing=self.render_studio.timeline.playing;
            if sui::icon_button(ui,if playing {Icon::Pause} else {Icon::Play},enabled,playing,"Play / pause (Space)").clicked() {
                self.render_studio.timeline.toggle_play();
            }
            if sui::icon_button(ui,Icon::StepForward,enabled,false,"Next frame (Right arrow)").clicked() {
                self.render_studio.timeline.seek(t+step);self.profile_preview_key=None;
            }
            if sui::icon_button(ui,Icon::ToEnd,enabled,false,"Go to OUT point").clicked() {
                let (_,end)=self.render_studio.timeline.export_range();
                self.render_studio.timeline.seek(end);self.profile_preview_key=None;
            }
        });
        sui::region(ui,row,"studio_transport_right",egui::Layout::right_to_left(egui::Align::Center),|ui| {
            ui.spacing_mut().item_spacing.x=2.0;
            ui.label(egui::RichText::new(sui::timecode(duration,fps)).monospace().size(12.5).color(theme::TEXT_FAINT))
                .on_hover_text(format!("Timeline duration {duration:.2} s"));
            ui.add_space(10.0);
            let snap=self.render_studio.timeline.snap;
            if sui::icon_button(ui,Icon::Snap,true,snap,if snap {"Snap to 30 FPS frames: on"} else {"Snap to 30 FPS frames: off"}).clicked() {
                self.render_studio.timeline.snap=!snap;
            }
            let free=!locked && self.render_studio.spatial.scene_camera.is_none();
            if sui::icon_button(ui,Icon::Frame,free,false,"Frame active actor with the shot camera").clicked() {
                self.render_studio_frame_actor();
            }
            if sui::icon_button(ui,Icon::AddKey,!locked,false,"Add camera key at playhead").clicked() {
                self.capture_studio_preview_camera(t);
            }
            // A local monitor control, not a track gain or export setting.
            // In this right-to-left region, the icon is visually left of the slider.
            ui.add_space(8.0);
            let mut volume=self.render_studio.preview_volume;
            ui.label(egui::RichText::new(format!("{:.0}%",volume*100.0)).size(11.5).color(theme::TEXT_DIM));
            ui.spacing_mut().slider_width=86.0;
            if ui.add(egui::Slider::new(&mut volume,0.0..=1.0).show_value(false))
                .on_hover_text(format!("Preview volume: {:.0}% (does not affect exports)",volume*100.0))
                .changed() {
                self.render_studio.preview_volume=volume;
            }
            let (icon_rect,icon_response)=ui.allocate_exact_size(egui::vec2(17.0,20.0),egui::Sense::hover());
            sui::paint_icon(ui.painter(),icon_rect.center(),Icon::Audio,theme::TEXT_DIM);
            icon_response.on_hover_text("Preview playback volume: demo audio and imported tracks only");
        });
    }

    /// Source-monitor transport while previewing a cinematic library clip.
    fn egui_studio_cinematic_transport(&mut self,ui:&mut egui::Ui,rect:egui::Rect) {
        let mut back=false;let mut add=None;let mut changed=false;
        sui::region(ui,rect.shrink2(egui::vec2(sui::PAD,0.0)),"studio_source_transport",
            egui::Layout::left_to_right(egui::Align::Center),|ui| {
            let Some(preview)=self.render_studio.cinematic_preview.as_mut() else {return;};
            if preview.playing && !preview.emote.looping && preview.time+preview.started.elapsed().as_secs_f64()>=preview.clip.duration() {
                preview.seek(preview.clip.duration());changed=true;
            }
            if sui::icon_button(ui,if preview.playing {Icon::Pause} else {Icon::Play},true,preview.playing,"Play / pause source (Space)").clicked() {
                preview.toggle();changed=true;
            }
            let duration=preview.clip.duration();let mut time=preview.elapsed();
            ui.label(egui::RichText::new(format!("{time:.2}")).monospace().size(13.0).color(theme::ACCENT));
            ui.spacing_mut().slider_width=(ui.available_width()-430.0).max(60.0);
            if ui.add(egui::Slider::new(&mut time,0.0..=duration).show_value(false)).changed() {
                preview.seek(time);changed=true;
            }
            ui.label(egui::RichText::new(format!("{duration:.2}s")).monospace().size(12.0).color(theme::TEXT_FAINT));
            ui.add_space(8.0);
            changed|=sui::toggle(ui,&mut preview.emote.looping).on_hover_text("Loop the source preview").changed();
            ui.label(egui::RichText::new("Loop").size(12.0).color(theme::TEXT_DIM));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui| {
                if sui::button(ui,Some(Icon::Plus),"Add at playhead",ButtonKind::Primary,true).clicked() {
                    add=Some((preview.clip.clone(),preview.lane));
                }
                if sui::button(ui,None,"Back to timeline",ButtonKind::Normal,true).clicked() {back=true;}
            });
        });
        if back {self.render_studio.cinematic_preview=None;changed=true;}
        if let Some((clip,lane))=add {
            let time=self.render_studio.timeline_time();
            self.render_studio_drop_media(StudioMediaDrag {name:clip.label,emote_id:None,cinematic:Some(clip.source),demo_game:None},time,lane);changed=true;
        }
        if changed {self.profile_preview_key=None;ui.ctx().request_repaint();}
    }

    fn egui_render_studio_library_footer(&mut self,ui:&mut egui::Ui,rect:egui::Rect) {
        let p=ui.painter();
        p.line_segment([rect.left_top(),rect.right_top()],egui::Stroke::new(1.0,theme::HAIRLINE));
        let text=match self.render_studio.media_tab {
            StudioMediaTab::Demos=>"Drag a demo onto the DEMO POV track",
            StudioMediaTab::Cutscenes=>"Click to preview · drag onto a character track",
            _=>"Drag onto a character track",
        };
        p.with_clip_rect(rect.shrink2(egui::vec2(sui::PAD,0.0))).text(egui::pos2(rect.left()+sui::PAD,rect.center().y),
            egui::Align2::LEFT_CENTER,text,egui::FontId::proportional(11.0),theme::TEXT_FAINT);
    }

    fn egui_render_studio_media_library(&mut self, ui: &mut egui::Ui, inspector_inline: bool) {
        sui::tab_strip(ui,&mut self.render_studio.media_tab,&[
            (StudioMediaTab::Jka,"Anims"),(StudioMediaTab::Fortnite,"Emotes"),
            (StudioMediaTab::Cutscenes,"Cinematics"),(StudioMediaTab::Demos,"Demos"),
        ]);
        ui.add_space(6.0);
        let locked=self.studio_locked();
        match self.render_studio.media_tab {
            StudioMediaTab::Jka => {
                sui::search_field(ui,&mut self.render_studio.animation_search,"Search JKA animations");
                let needle=self.render_studio.animation_search.trim().to_ascii_lowercase();
                let all:Vec<&'static str>=(0..1600).filter_map(jka_movement::animation_name)
                    .filter(|name|needle.is_empty()||name.to_ascii_lowercase().contains(&needle)).collect();
                ui.add_space(2.0);
                sui::hint(ui,&if all.len()>150 {format!("{} matches · showing first 150",all.len())}
                    else {format!("{} animations",all.len())});
                self.studio_media_list(ui,"studio_jka_list",inspector_inline,|app,ui| {
                    if all.is_empty(){sui::hint(ui,"No matching animations.");}
                    for name in all.iter().take(150) {
                        app.egui_render_studio_media_tile(ui,StudioMediaDrag {name:(*name).to_owned(),emote_id:None,cinematic:None,demo_game:None},locked);
                    }
                });
            }
            StudioMediaTab::Fortnite => {
                sui::search_field(ui,&mut self.render_studio.emote_search,"Search Fortnite emotes");
                let needle=self.render_studio.emote_search.trim().to_ascii_lowercase();
                let choices=crate::emotes::catalog(&self.base).map(|catalog| catalog.iter().filter(|emote|
                    needle.is_empty()||emote.label.to_ascii_lowercase().contains(&needle)
                    ||emote.id.to_ascii_lowercase().contains(&needle))
                    .take(150).map(|emote| {
                        let variant=crate::emotes::default_variant_index(emote);
                        let resolved=emote.variants.get(variant).unwrap_or(emote);
                        StudioMediaDrag {name:crate::emotes::display_title(emote).to_owned(),emote_id:Some(resolved.id.clone()),cinematic:None,demo_game:None}
                    }).collect::<Vec<_>>());
                ui.add_space(2.0);
                self.studio_media_list(ui,"studio_emote_list",inspector_inline,|app,ui| match choices {
                    Ok(choices) => {
                        if choices.is_empty(){sui::hint(ui,"No matching emotes.");}
                        for choice in choices {app.egui_render_studio_media_tile(ui,choice,locked);}
                    }
                    Err(_)=>sui::callout(ui,"Fortnite emote catalog unavailable.",theme::WARNING),
                });
            }
            StudioMediaTab::Cutscenes => self.studio_media_list(ui,"studio_cinematic_list",inspector_inline,
                |app,ui| app.egui_studio_cinematic_library(ui,locked)),
            StudioMediaTab::Demos => self.studio_media_list(ui,"studio_demo_list",inspector_inline,
                |app,ui| app.egui_render_studio_demo_library(ui,locked)),
        }
    }

    /// Scrolling browser body. On narrow windows the inspector has no pane of
    /// its own and folds in under the list.
    fn studio_media_list(&mut self,ui:&mut egui::Ui,id:&str,inspector_inline:bool,
        add:impl FnOnce(&mut Self,&mut egui::Ui)) {
        egui::ScrollArea::vertical().id_salt(id).auto_shrink([false,false]).show(ui,|ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y=1.0;
            if ui.ui_contains_pointer() {self.render_studio.library_hovered=true;}
            add(self,ui);
            if inspector_inline {
                ui.spacing_mut().item_spacing.y=5.0;
                ui.add_space(8.0);
                sui::section(ui,"studio_inline_inspector","Inspector",None,true,|ui| self.egui_render_studio_inspector(ui));
            }
        });
    }

    fn egui_render_studio_demo_library(&mut self,ui:&mut egui::Ui,locked:bool) {
        self.ensure_demo_catalog();
        if let Some(error)=&self.demo_catalog_error {
            sui::callout(ui,&format!("Demo scan failed: {error}"),theme::WARNING);return;
        }
        ui.spacing_mut().item_spacing.y=5.0;
        let games=self.demo_game_choices.clone();
        let mut changed=false;
        sui::prop_row(ui,"Game folder",|ui| {
            sui::combo_box("studio_demo_game").width(ui.available_width())
                .selected_text(if self.demo_game_filter.is_empty(){"base"}else{&self.demo_game_filter})
                .show_ui(ui,|ui| {
                    for game in &games {
                        let value=if game.eq_ignore_ascii_case("base"){String::new()}else{game.clone()};
                        if ui.selectable_value(&mut self.demo_game_filter,value,game).changed(){changed=true;}
                    }
                });
        });
        if changed {
            self.render_studio.pending_demo_drop=None;
            self.reset_demo_browser_catalog();self.ensure_demo_catalog();
        }
        ui.add_enabled_ui(!locked, |ui| {
            ui.checkbox(&mut self.render_studio.keep_demos_loaded, "Keep demos loaded")
                .on_hover_text("Keep each recording loaded across cuts and Director shots. Same-recording fades can use a second playback context sharing the recording bytes. Inactive views release their large screen-sized targets. Turn off to retain only current/next recordings.");
        });
        let loaded = self.render_studio.sources.iter().filter(|s| s.gpu_ready && !s.failed).count()
            + self.render_studio.source_prepares.iter().filter(|job| !job.upload && !job.cancelled).count();
        let recordings: std::collections::HashSet<_> = self.render_studio.sources.iter().filter(|s| s.gpu_ready && !s.failed)
            .map(|s| (s.name.to_ascii_lowercase(), s.game.to_ascii_lowercase()))
            .chain(self.render_studio.source_prepares.iter().filter(|j| !j.upload && !j.cancelled)
                .map(|j| (j.name.to_ascii_lowercase(), j.game.to_ascii_lowercase()))).collect();
        let wanted: std::collections::HashSet<_> = self.render_studio.timeline.demos.iter()
            .map(|c| (c.name.to_ascii_lowercase(), c.game.to_ascii_lowercase())).collect();
        let pending = self.render_studio.source_job.iter().chain(&self.render_studio.source_prepares)
            .filter(|j| !j.cancelled).count()
            + self.render_studio.sources.iter().filter(|s| !s.gpu_ready && !s.failed).count();
        let mut status = format!("{} of {} recordings loaded · {loaded} playback contexts",
            recordings.len().min(wanted.len()), wanted.len());
        if pending > 0 { status.push_str(" · loading…"); }
        if self.render_studio.scene_capacity != usize::MAX {
            status.push_str(&format!(" · GPU memory limit: {} kept", self.render_studio.scene_capacity.max(1)));
        }
        ui.label(egui::RichText::new(status).small().color(theme::TEXT_FAINT));
        ui.add_enabled_ui(!locked,|ui| {
            if sui::button(ui,Some(Icon::Open),"Import DM26 from PC…",ButtonKind::Primary,true).clicked() {
                self.render_studio.file_picker.open(crate::app::render_studio_picker::StudioPickerKind::Demo);
            }
        });
        sui::search_field(ui,&mut self.render_studio.demo_search,"Search demos");
        if self.demo_catalog_pending {
            ui.horizontal(|ui|{ui.spinner();sui::hint(ui,"Scanning demo folders…");});
            return;
        }
        let needle=self.render_studio.demo_search.trim().to_ascii_lowercase();
        let names:Vec<_>=self.demo_entries.iter().map(|entry|entry.demo_name.clone())
            .filter(|name|needle.is_empty()||name.to_ascii_lowercase().contains(&needle)).collect();
        sui::hint(ui,&if names.is_empty() {"No demos found in this game folder.".to_owned()}
            else if names.len()>250 {format!("{} demos · showing first 250",names.len())}
            else {format!("{} demos",names.len())});
        ui.spacing_mut().item_spacing.y=1.0;
        // Draw only actual visible rows so large folders do not allocate a giant
        // widget tree. Metadata remains lazy, indexed on click or drop.
        ui.add_enabled_ui(!locked,|ui| {
            for name in names.iter().take(250) {
                let row=self.egui_render_studio_media_tile(ui,StudioMediaDrag {
                    name:name.clone(),emote_id:None,cinematic:None,
                    demo_game:Some(self.demo_game_filter.clone()),
                },locked);
                // Keep the media browser focused on choosing recordings. A
                // right-click assigns one to any specific demo timeline clip.
                row.context_menu(|ui| {
                    sui::editor_style(ui);
                    ui.set_min_width(260.0);
                    ui.menu_button("Add as composite to…", |ui| {
                        ui.set_min_width(280.0);
                        let targets = self.render_studio.timeline.demos.iter().enumerate()
                            .map(|(i,clip)| (i,clip.name.clone(),clip.game.clone(),clip.start,clip.layer,
                                clip.race_overlays.iter().any(|o|o.name==*name && o.game==self.demo_game_filter)))
                            .collect::<Vec<_>>();
                        if targets.is_empty() { ui.label("Drag a main demo onto the timeline first."); }
                        for (i,target,game,start,layer,included) in targets {
                            let folder=if game.is_empty(){"base"}else{&game};
                            let label=format!("{} · {} · L{} · {}",target,folder,layer+1,format_seconds(start));
                            if ui.selectable_label(included,label).on_hover_text(
                                if included {"Click to detach from this demo clip"}
                                else {"Composite this recording into this demo clip"}
                            ).clicked() {
                                self.toggle_studio_race_overlay_for_demo(i,name.clone(),self.demo_game_filter.clone());
                                self.render_studio.timeline.clear_selection();
                                self.render_studio.timeline.selected_demo=Some(i);
                                self.render_studio.audio.selected_demo_audio=None;
                                ui.close();
                            }
                        }
                    });
                });
            }
        });
        let total: usize=self.render_studio.timeline.demos.iter()
            .map(|d|d.race_overlays.len()).sum();
        if total>0 {
            ui.add_space(7.0);
            sui::hint(ui,&format!("{total} composite assignments · Select a demo clip to edit its race actors in Inspector."));
        }
    }

    /// The selected DEMO POV clip owns these references. No new source or
    /// renderer is created here: all actors still use the existing /rGhost
    /// track decoder and playback-time alignment.
    fn egui_studio_composite_inspector(&mut self,ui:&mut egui::Ui,clip_index:usize) {
        if self.render_studio.timeline.demos.get(clip_index).is_none(){return;}
        if self.render_studio.timeline.demos[clip_index].comparison.is_some() {
            let clip=&mut self.render_studio.timeline.demos[clip_index];
            let reference=clip.comparison.as_ref().map(|race| {
                let runner=&race.runners[race.reference];(runner.name.clone(),runner.game.clone())
            });
            sui::callout(ui,"All runners remain active throughout this ghost race. Add recordings to the timeline, then regenerate in Director to change the group.",theme::ACCENT);
            if let Some(race)=clip.comparison.as_mut() {
                studio_overlay_style_section(ui,&mut race.style,&mut self.render_studio.overlay_fonts,
                    &mut self.render_studio.overlay_fonts_job,&mut self.render_studio.overlay_font_filter);
            }
            sui::section(ui,"studio_ghost_race_roster","Ghost race runners",None,true,|ui| {
                for (i,setting) in clip.race_overlays.iter_mut().enumerate() {
                    ui.push_id(("comparison_runner",i),|ui| {
                        ui.label(&setting.name);
                        if reference.as_ref().is_some_and(|(name,game)|*name==setting.name&&*game==setting.game) {
                            sui::prop_value(ui,"Opacity","Opaque during main chase");
                        }else{
                            sui::prop_slider(ui,"Opacity",egui::Slider::new(&mut setting.alpha,0.25..=1.0));
                        }
                        ui.checkbox(&mut setting.strafe_trail,"Strafe trail");
                        ui.checkbox(&mut setting.trail_ticks,"1-second tick marks");
                        studio_trail_label_picker(ui,("comparison",clip_index,i),&mut setting.trail_tick_labels);
                        studio_trail_color_picker(ui,setting.auto_trail_color(),&mut setting.trail_color);
                    });
                }
            });
            if let Some(race)=clip.comparison.as_mut() {
                for runner in &mut race.runners {
                    if let Some(o)=clip.race_overlays.iter().find(|o|o.name==runner.name&&o.game==runner.game) {
                        runner.color=o.resolved_trail_color();
                    }
                }
            }
            return;
        }
        self.ensure_demo_catalog();
        let before_enabled=self.render_studio.timeline.demos.iter()
            .any(|d|d.race_overlays.iter().any(|o|o.enabled));
        let mut enabled_toggled = false;
        sui::section(ui,"studio_insp_demo_own_trail","Recorded POV strafe trail",
            Some("Draw the selected recording's own duelTime race route, even with a detached camera. Independent of composite actors and gameplay cvars."),true,|ui| {
            let clip=&mut self.render_studio.timeline.demos[clip_index];
            let auto_color=crate::app::render_studio_timeline::studio_name_trail_color(&clip.name,&clip.game);
            let setting=&mut clip.pov_trail;
            ui.checkbox(&mut setting.enabled,"Show recorded POV trail");
            if setting.enabled {
                ui.checkbox(&mut setting.trail_ticks,"1-second tick marks")
                    .on_hover_text("Real duelTime seconds from the recorded race, not the Studio timeline clock.");
                ui.add_enabled_ui(setting.trail_ticks,|ui| {
                    studio_trail_label_picker(ui,("pov",clip_index),&mut setting.trail_tick_labels);
                });
                studio_trail_color_picker(ui,auto_color,&mut setting.trail_color);
                if let Some(runtime)=self.render_studio.race_runtime.iter().find(|r|
                    r.key==(clip.name.clone(),clip.game.clone())) {
                    if let Some(error)=&runtime.error{sui::callout(ui,error,theme::WARNING);}
                    else if runtime.ghost.is_some(){sui::hint(ui,"Race route ready");}
                    else{sui::hint(ui,"Decoding POV race route…");}
                } else {sui::hint(ui,"Waiting for race route…");}
            }
        });
        sui::section(ui,"studio_insp_demo_composite","Composite race actors",
            Some("Only shown while this demo clip is being rendered. Other demos supply their player models, not a second BSP. Synchronization uses the race-start duelTime anchor."),true,|ui| {
            let count=self.render_studio.timeline.demos[clip_index].race_overlays.len();
            sui::hint(ui,&format!("{count} attached · race time synchronized"));
            ui.menu_button("+ Add composite demo…", |ui| {
                sui::editor_style(ui);
                ui.set_min_width(260.0);
                let games=self.demo_game_choices.clone();
                let mut changed=false;
                ui.horizontal(|ui| {
                    ui.label("Source folder");
                    egui::ComboBox::from_id_salt(("studio_composite_game",clip_index))
                        .selected_text(if self.demo_game_filter.is_empty(){"base"}else{&self.demo_game_filter})
                        .show_ui(ui,|ui| {
                            for game in &games {
                                let value=if game.eq_ignore_ascii_case("base"){String::new()}else{game.clone()};
                                if ui.selectable_value(&mut self.demo_game_filter,value,game).changed(){changed=true;}
                            }
                        });
                });
                if changed { self.reset_demo_browser_catalog();self.ensure_demo_catalog(); }
                ui.add(egui::TextEdit::singleline(&mut self.render_studio.composite_search)
                    .hint_text("Search source recordings…"));
                if self.demo_catalog_pending {ui.horizontal(|ui|{ui.spinner();ui.label("Scanning recordings…");});}
                let filter=self.render_studio.composite_search.trim().to_ascii_lowercase();
                let names=self.demo_entries.iter().map(|e|e.demo_name.clone())
                    .filter(|n|filter.is_empty()||n.to_ascii_lowercase().contains(&filter))
                    .take(100).collect::<Vec<_>>();
                let game=self.demo_game_filter.clone();
                egui::ScrollArea::vertical().id_salt(("studio_composite_picker",clip_index))
                    .max_height(240.0).show(ui,|ui| {
                        for name in names {
                            let added=self.render_studio.timeline.demos[clip_index].race_overlays.iter()
                                .any(|o|o.name==name && o.game==game);
                            if ui.add_enabled(!added,egui::Button::new(format!("{}{}",if added{"ADDED  "}else{""},name)))
                                .on_hover_text(if added {"Already attached"}else{"Add this recording's race actor"}).clicked() {
                                self.toggle_studio_race_overlay_for_demo(clip_index,name,game.clone());
                                ui.close();
                            }
                        }
                    });
                if self.demo_entries.is_empty()&&!self.demo_catalog_pending {
                    sui::hint(ui,"No recordings in this folder. Choose another source folder.");
                }
            });
            let mut remove=None;
            let mut state_changed=false;
            let len=self.render_studio.timeline.demos[clip_index].race_overlays.len();
            for index in 0..len {
                let setting=&mut self.render_studio.timeline.demos[clip_index].race_overlays[index];
                ui.push_id(("clip_composite",clip_index,index),|ui| {
                    ui.horizontal(|ui| {
                        state_changed |= sui::toggle(ui,&mut setting.enabled).changed();
                        ui.add(egui::Label::new(egui::RichText::new(&setting.name).size(12.5).color(theme::TEXT)).truncate());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui| {
                            if sui::icon_button(ui,Icon::Close,true,false,"Remove from this demo").clicked(){remove=Some((setting.name.clone(),setting.game.clone()));}
                        });
                    });
                    sui::prop_slider(ui,"Opacity",egui::Slider::new(&mut setting.alpha,0.02..=1.0).fixed_decimals(2));
                    ui.checkbox(&mut setting.strafe_trail,"Strafe trail")
                        .on_hover_text("Reuse the race ghost's indexed route and strafe trail rendering for this composite actor.");
                    ui.add_enabled_ui(setting.strafe_trail,|ui| {
                        ui.checkbox(&mut setting.trail_ticks,"1-second tick marks")
                            .on_hover_text("Perpendicular notches located at real race-clock seconds, not estimated from playback FPS.");
                        ui.add_enabled_ui(setting.trail_ticks,|ui| {
                            studio_trail_label_picker(ui,("composite",clip_index,index),&mut setting.trail_tick_labels);
                        });
                        studio_trail_color_picker(ui,setting.auto_trail_color(),&mut setting.trail_color);
                    });
                    if let Some(runtime)=self.render_studio.race_runtime.iter().find(|r|r.key==(setting.name.clone(),setting.game.clone())) {
                        if let Some(error)=&runtime.error{sui::callout(ui,error,theme::WARNING);}
                        else if runtime.ghost.is_some(){sui::hint(ui,"Ready · synced to race reset");}
                        else{sui::hint(ui,"Decoding local recording…");}
                    }else if setting.enabled{sui::hint(ui,"Waiting for race source…");}
                });
            }
            if state_changed {
                self.render_studio.race_runtime.clear();
                enabled_toggled = true;
            }
            if let Some((name,game))=remove {
                self.toggle_studio_race_overlay_for_demo(clip_index,name,game);
            }
        });
        if enabled_toggled && !before_enabled && self.render_studio.timeline.demos.iter()
            .any(|d|d.race_overlays.iter().any(|o|o.enabled)) {
            self.invalidate_studio_sources();
        }
    }

    /// Native Explorer picker imports recordings from any readable disk location.
    pub(in crate::app) fn studio_import_demo_files(&mut self, files:Vec<std::path::PathBuf>) {
        let mut count=0usize;
        for source in files {
            if !source.extension().and_then(|e|e.to_str()).is_some_and(|e|e.eq_ignore_ascii_case("dm_26")) {
                self.render_studio.status=format!("Not a DM26 demo: {}",source.display());continue;
            }
            let bytes=match std::fs::read(&source) {
                Ok(bytes) if !bytes.is_empty() && bytes.len()<=512*1024*1024 =>bytes,
                Ok(_)=>{self.render_studio.status="Demo must be 1 byte to 512 MiB".into();continue;},
                Err(error)=>{self.render_studio.status=format!("Demo read failed: {error}");continue;},
            };
            let duration=match crate::app::build_demo_index(&bytes) {
                Ok(index) if (1..=36_000_000).contains(&index.duration_ms())=>index.duration_ms(),
                Ok(_)=>{self.render_studio.status="Demo has no valid active POV duration".into();continue;},
                Err(error)=>{self.render_studio.status=format!("Demo indexing failed: {error}");continue;},
            };
            let owned=match self.studio_copy_demo_bytes(&source,&bytes) {
                Ok(relative)=>relative,
                Err(error)=>{self.render_studio.status=format!("Demo import failed: {error}");continue;},
            };
            // The hash suffix distinguishes recordings with identical filenames.
            let name=std::path::Path::new(&owned).file_stem()
                .map(|s|s.to_string_lossy().into_owned()).unwrap_or_else(||"imported".into());
            let game=String::new(); // Actual mod assets are resolved from the embedded demo gamestate.
            let layer=0usize;
            let at=self.render_studio.timeline.demos.iter().filter(|c|c.layer==layer)
                .map(|c|c.end()).fold(self.render_studio.timeline_time(),f64::max);
            self.render_studio_place_demo(name,game,at,duration,layer,Some(owned));
            count+=1;
        }
        if count>0 {self.render_studio.status=format!("Imported {count} DM26 demo(s) into project media");}
    }

    pub(in crate::app) fn render_studio_queue_demo_drop(&mut self,name:String,game:String,time:f64,layer:usize) {
        if game!=self.demo_game_filter {
            self.render_studio.status="Demo directory changed while dragging; retry the drop".into();return;
        }
        if let Some(metadata)=self.demo_metadata_cache.get(&name) {
            self.render_studio_place_demo(name,game,time,metadata.duration_ms,layer,None);
        } else if let Some(error)=self.demo_metadata_errors.get(&name) {
            self.render_studio.status=format!("Cannot add demo: {error}");
        } else {
            self.ensure_demo_metadata(&name);
            self.render_studio.pending_demo_drop=Some((name,game,time,layer));
            self.render_studio.status="Indexing demo to determine its real duration…".into();
        }
    }
    fn render_studio_place_demo(&mut self,name:String,game:String,time:f64,duration_ms:i32,layer:usize,imported:Option<String>) {
        if !(1..=36_000_000).contains(&duration_ms) {
            self.render_studio.status="Demo contains no usable active POV snapshots, or exceeds the editor's 10-hour limit".into();return;
        }
        let source_path=match imported {
            Some(path)=>path,
            None=>match self.studio_copy_library_demo(&name,&game) {
                Ok(path)=>path,
                Err(error)=>{self.render_studio.status=format!("Cannot copy demo into project: {error}");return;}
            },
        };
        let clip=StudioDemoClip { source_path: Some(source_path), comparison: None, race_overlays: Vec::new(), pov_trail:Default::default(), hud_overrides:Default::default(), hide_other_players:false,name,game,start:time,duration_ms,in_ms:0,out_ms:duration_ms,speed_keys:vec![],transition_in:Default::default(),audio:Default::default(),layer};
        if !clip.valid() {
            self.render_studio.status="Invalid demo source path or metadata".into();return;
        }
        self.render_studio.timeline.add_demo(clip);
        // With no animation on the timeline, stage-camera keys would show an
        // empty stage over the demo: switch them to the default demo camera.
        if self.render_studio.timeline.clips.is_empty() {
            let fov=self.studio_default_fov();
            for key in &mut self.render_studio.timeline.camera {
                if matches!(key.source,crate::app::render_studio_timeline::StudioCameraSource::Animation) {
                    Self::studio_default_demo_camera(key);
                    if key.fov==58.0 {key.fov=fov;}
                }
            }
        }
        self.render_studio.selected_background=false;
        self.render_studio.status="Demo footage added. Move the playhead onto it to preview the native BSP/POV here.".into();
        self.profile_preview_key=None;
    }
    fn render_studio_open_demo_pov(&mut self,index:usize,time:f64) {
        let Some(clip)=self.render_studio.timeline.demos.get(index).cloned() else {return;};
        self.render_studio.timeline.seek(time.clamp(clip.start,(clip.end()-0.001).max(clip.start)));
        // Actual session loading/seek happens in the regular App tick,
        // not synchronously inside egui paint (which can block input).
        self.render_studio.timeline.playing=false;
        // An explicit preview click retries a failed source even if the editor
        // remains parked at exactly the same timeline timestamp.
        if self.render_studio.demo_preview.as_ref().is_some_and(|p|p.failed) {
            self.render_studio.demo_preview = None;
        }
        self.egui_repaint_requested=true;
    }

    /// Library click: Ctrl toggles a row, Shift selects the rows between the
    /// anchor and this one (inclusive, in list order).
    fn studio_library_click(&mut self,key:String,ctrl:bool,shift:bool) {
        let rows:Vec<String>=self.render_studio.library_rows.iter().map(|(k,_)|k.clone()).collect();
        let anchor=self.render_studio.library_anchor.clone().filter(|a|rows.contains(a));
        let mut selected=if ctrl {self.render_studio.library_selection.clone()} else {Vec::new()};
        match (shift,anchor.as_ref().and_then(|a|rows.iter().position(|k|k==a)),rows.iter().position(|k|*k==key)) {
            (true,Some(a),Some(b)) => {
                for k in &rows[a.min(b)..=a.max(b)] {if !selected.contains(k) {selected.push(k.clone());}}
            }
            _ if ctrl && selected.contains(&key) => selected.retain(|k|*k!=key),
            _ => if !selected.contains(&key) {selected.push(key.clone());},
        }
        self.render_studio.library_selection=selected;
        if !shift || anchor.is_none() {self.render_studio.library_anchor=Some(key);}
    }

    /// One browser row per source. Returns the row response so callers can
    /// layer row actions (e.g. the race-actor button) over it.
    pub(super) fn egui_render_studio_media_tile(&mut self,ui:&mut egui::Ui,asset:StudioMediaDrag,locked:bool)->egui::Response {
        let (icon,tint)=if asset.demo_game.is_some() {(Icon::Demo,sui::DEMO)}
            else if asset.cinematic.is_some() {(Icon::Cinematic,theme::WARNING)}
            else if asset.emote_id.is_some() {(Icon::Emote,sui::SPEED)}
            else {(Icon::Anim,theme::ACCENT)};
        let meta=if asset.demo_game.is_some() {
            if let Some(meta)=self.demo_metadata_cache.get(&asset.name) {Self::format_demo_time(meta.duration_ms)}
            else if self.demo_metadata_inflight.contains(&asset.name) {"indexing…".to_owned()} else {String::new()}
        } else {String::new()};
        let key=studio_library_key(&asset);
        self.render_studio.library_rows_next.push((key.clone(),asset.clone()));
        let dragging=self.render_studio.media_drag.as_ref().is_some_and(|d|d.name==asset.name)
            || self.render_studio.media_drag_group.iter().any(|d|d.name==asset.name);
        let picked=self.render_studio.library_selection.contains(&key);
        let r=sui::list_row(ui,icon,tint,&asset.name,&meta,dragging||picked,
            if locked {egui::Sense::hover()} else {egui::Sense::click_and_drag()});
        if r.clicked() {
            let (ctrl,shift)=ui.input(|i|(i.modifiers.ctrl||i.modifiers.command,i.modifiers.shift));
            self.studio_library_click(key.clone(),ctrl,shift);
        }
        if r.clicked() {
            if asset.demo_game.is_some() {self.ensure_demo_metadata(&asset.name);}
            if let Some(source)=asset.cinematic.as_ref() {
                ui.ctx().memory_mut(|m|if let Some(id)=m.focused(){m.surrender_focus(id);});
                self.preview_studio_cinematic(source);
            }
        }
        if r.drag_started() {
            // Dragging one of several selected rows drags them all (same kind).
            self.render_studio.media_drag_group.clear();
            if picked && self.render_studio.library_selection.len()>1 {
                let demo=asset.demo_game.is_some();
                let group:Vec<StudioMediaDrag>=self.render_studio.library_rows.iter()
                    .filter(|(k,a)|*k!=key && self.render_studio.library_selection.contains(k) && a.demo_game.is_some()==demo)
                    .map(|(_,a)|a.clone()).collect();
                for item in &group {if item.demo_game.is_some() {self.ensure_demo_metadata(&item.name);}}
                self.render_studio.media_drag_group=group;
            } else {
                self.render_studio.library_selection=vec![key.clone()];
                self.render_studio.library_anchor=Some(key.clone());
            }
            self.render_studio.cinematic_preview=None;self.profile_preview_key=None;
            if asset.demo_game.is_some() {self.ensure_demo_metadata(&asset.name);}
            self.render_studio.media_drag_duration=asset.emote_id.as_deref()
                .and_then(|id|crate::emotes::catalog(&self.base).ok().and_then(|catalog| {
                    catalog.iter().flat_map(|root|std::iter::once(root).chain(root.variants.iter()))
                        .find(|candidate|candidate.id==id).cloned()
                }))
                .and_then(|variant|crate::emotes::prepare_visuals(variant).ok())
                .map(|(emote,_)|(emote.end_frame-emote.first_frame) as f64/
                    f64::from(emote.fps).max(0.001));
            self.render_studio.media_drag=Some(asset.clone());
        }
        if r.dragged() {ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);}
        let details=asset.cinematic.as_ref().and_then(|source|self.render_studio.cinematics.catalog.as_ref()
            .and_then(|r|r.as_ref().ok()).and_then(|catalog|catalog.clips.iter().find(|clip|clip.source==*source)))
            .map(|clip|format!("{}\n{} / {}\n{} frames, {:.2} FPS, source loopFrames {}\nClick to preview; drag onto a character track",clip.label,
                clip.source.gla,clip.source.animation,clip.frame_count,clip.fps,clip.loop_frames))
            .unwrap_or_else(||if let Some(meta)=asset.demo_game.as_ref().and_then(|_|self.demo_metadata_cache.get(&asset.name)) {
                format!("demos/{}.dm_26\n{} · {} snapshots · {}\nDrag onto the DEMO POV track.",asset.name,
                    Self::format_demo_time(meta.duration_ms),meta.snapshot_count,
                    meta.recorded_player_name.as_deref().unwrap_or("POV unknown"))
            } else if asset.demo_game.is_some() {
                format!("demos/{}.dm_26\nClick to index its duration; drag onto the DEMO POV track.",asset.name)
            } else {format!("{}\nDrag onto a character track",asset.name)});
        r.on_hover_text(details)
    }

    fn egui_render_studio_output(&mut self,ui:&mut egui::Ui) {
        let locked=self.studio_locked();
        let fps_before=self.render_studio.fps;
        ui.add_enabled_ui(!locked, |ui| {
            sui::section(ui,"studio_export_renderer","Renderer",
                Some("Preview and export use the Studio graphics profile, separate from your gameplay settings. The gear opens its settings."),true,|ui| {
                sui::prop_row(ui,"Graphics profile",|ui| {
                    let w=(ui.available_width()-34.0).max(80.0);
                    self.egui_studio_profile_picker(ui,locked,w);
                });
            });
            sui::section(ui,"studio_export_video","Video",None,true,|ui| {
                let previous_format = self.render_studio.preset;
                let formats:Vec<_>=StudioVideoFormat::OPTIONS.iter().map(|f|(*f,f.label())).collect();
                sui::prop_combo(ui,"Format","render_studio_quality",&mut self.render_studio.preset,&formats);
                if self.render_studio.preset != previous_format {
                    // The H.264 format names are quality starting points.
                    match self.render_studio.preset {
                        StudioVideoFormat::H264Max => self.render_studio.crf = 12,
                        StudioVideoFormat::H264High => self.render_studio.crf = crate::app::render_studio::STUDIO_DEFAULT_CRF,
                        _ => {}
                    }
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
                if matches!(self.render_studio.preset, StudioVideoFormat::H264High | StudioVideoFormat::H264Max) {
                    let default = if self.render_studio.preset == StudioVideoFormat::H264Max {"Default (slow)"} else {"Default (medium)"};
                    let speeds:Vec<_>=StudioEncoderSpeed::OPTIONS.iter().map(|speed|
                        (*speed,speed.x264().unwrap_or(default))).collect();
                    sui::prop_combo(ui,"Encoder speed","render_studio_encoder_speed",&mut self.render_studio.encoder_speed,&speeds);
                    if self.render_studio.encoder_speed != StudioEncoderSpeed::FormatDefault {
                        sui::prop_row(ui,"",|ui| sui::hint(ui,"x264 preset only: quality (CRF) is unchanged. Faster presets export quicker but give larger files; slower ones the reverse."));
                    }
                }
                if matches!(self.render_studio.preset, StudioVideoFormat::H264High | StudioVideoFormat::H264Max)
                    || self.render_studio.preset.nvenc_codec().is_some() {
                    sui::prop_slider(ui,"Quality (CRF)",egui::Slider::new(&mut self.render_studio.crf,8..=35)
                        .custom_formatter(|value,_| {
                            let crf=value.round() as u8;
                            format!("{crf} · {}",crate::app::render_studio::studio_crf_level(crf))
                        }))
                        .on_hover_text("Lower = better quality and bigger files. About 12 Max, 18 High (default), 23 Medium, 28 Low. NVENC formats use the matching CQ.");
                }
                if self.render_studio.preset.nvenc_codec().is_some() {
                    let presets:&[(u8,&str)]=&[(0,"Default (p5)"),(1,"p1 (fastest)"),(2,"p2"),(3,"p3"),
                        (4,"p4"),(5,"p5"),(6,"p6"),(7,"p7 (slowest)")];
                    sui::prop_combo(ui,"Encoder speed","render_studio_nvenc_preset",&mut self.render_studio.nvenc_preset,presets);
                    sui::prop_row(ui,"",|ui| sui::hint(ui,"Encoded on the NVIDIA GPU, leaving CPU cores to the export. Quality (CQ) is fixed; slower presets compress a little better."));
                }
                let rates:&[(u32,&str)]=if self.render_studio.preset == StudioVideoFormat::Gif {
                    &[(10,"10"),(15,"15"),(24,"24"),(30,"30")]
                } else {&[(24,"24"),(30,"30"),(60,"60"),(120,"120")]};
                sui::prop_segmented(ui,"Frame rate",&mut self.render_studio.fps,rates);
                let sizes:Vec<_>=StudioExportSize::OPTIONS.iter().map(|s|(*s,s.label())).collect();
                sui::prop_combo(ui,"Resolution","render_studio_export_size",&mut self.render_studio.export_size,&sizes);
                if self.render_studio.export_size.dimensions().is_some() {
                    sui::prop_row(ui,"",|ui| sui::hint(ui,"Demo/BSP footage renders offscreen at this size in any window mode; the viewer is letterboxed to match."));
                }
                sui::prop_row(ui,"",|ui| sui::hint(ui,self.render_studio.preset.short_description()));
                let has_demos = !self.render_studio.timeline.demos.is_empty();
                let can_export_alpha = self.render_studio.preset.supports_alpha() && !has_demos;
                ui.add_enabled_ui(can_export_alpha, |ui| {
                    sui::prop_toggle(ui,"Transparent",&mut self.render_studio.transparent)
                        .on_hover_text("Remove the background and keep alpha in the video");
                });
                if !can_export_alpha { self.render_studio.transparent = false; }
                sui::prop_toggle(ui,"Depth EXR",&mut self.render_studio.export_depth)
                    .on_hover_text("Also write a 32-bit float EXR depth sequence: camera-to-surface radial JKA units, empty pixels are 1,000,000.");
                if has_demos {sui::callout(ui,"Native demo footage exports opaque; recorded audio is not exported yet.",theme::WARNING);}
                if self.render_studio.transparent {
                    sui::hint(ui,match self.render_studio.preset {
                        StudioVideoFormat::Gif => "GIF alpha is 1-bit (threshold 128): soft hair and glow edges become solid or transparent. GIF loops forever.",
                        StudioVideoFormat::WebmVp9 | StudioVideoFormat::WebmVp9Max => "True alpha via VP9 WebM; the selected background is excluded. Some players show transparent areas as black.",
                        _ => "True alpha in ProRes 4444 MOV; the selected background is excluded from capture.",
                    });
                } else if self.render_studio.preset == StudioVideoFormat::Gif {
                    sui::hint(ui,"Opaque looping GIF with its backdrop. Large windows produce very large GIF files.");
                } else if !has_demos {
                    sui::hint(ui,"Character clips render on the stage. Video export is currently silent.");
                }
            });
            sui::section(ui,"studio_export_still","Still image",
                Some("Capture the current animation or recorded demo POV without editor UI. Demo PNGs are opaque."),true,|ui| {
                sui::prop_toggle(ui,"Transparent PNG",&mut self.render_studio.screenshot_alpha);
                sui::prop_toggle(ui,"Depth EXR",&mut self.render_studio.screenshot_depth);
            });
            let (begin,end)=self.render_studio.timeline.export_range();
            let fps=self.render_studio.fps;
            sui::prop_value(ui,"Range",&format!("{}  –  {}   ({})",sui::timecode(begin,fps),sui::timecode(end,fps),format_seconds(end-begin)));
        });
        if self.render_studio.fps!=fps_before {self.profile_preview_key=None;}
        ui.add_space(10.0);
        if locked {
            if self.render_studio.still.is_some() { sui::hint(ui,"Capturing still screenshot…"); }
            if let Some(export) = self.render_studio.export.as_ref() {
                ui.add(egui::ProgressBar::new(export.frame_index as f32 / export.total_frames.max(1) as f32)
                    .text(format!("{} / {} frames", export.frame_index, export.total_frames)));
            }
            if let Some(capture) = self.render_studio.dm26_capture.as_ref() {
                ui.add(egui::ProgressBar::new(capture.index as f32 / capture.total.max(1) as f32)
                    .text(format!("DM26 camera {} / {} frames", capture.index, capture.total)));
            }
            if sui::button_row(ui,&[(None,"Cancel export",ButtonKind::Danger,true)]).is_some() {
                if self.render_studio.dm26_capture.is_some() {
                    self.cancel_render_studio_dm26_export("DM26 export cancelled");
                } else if self.render_studio.still.is_some() {
                    // The in-flight readback can still finish, but no PNG is written.
                    self.render_studio.still = None;
                    self.asset_preview_viewport_key = None;
                    self.render_command(crate::app::RenderCommand::SetAssetPreviewViewport(None));
                } else { self.cancel_render_studio_export(); }
            }
        } else {
            let can_export=!self.render_studio.timeline.clips.is_empty() || !self.render_studio.timeline.demos.is_empty();
            match sui::button_row(ui,&[(Some(Icon::Still),"Capture still PNG",ButtonKind::Normal,true),
                (Some(Icon::Export),"Export video",ButtonKind::Primary,can_export)]) {
                Some(0)=>self.start_render_studio_screenshot(),
                Some(1)=>self.start_render_studio_export(),
                _=>{}
            }
        }
        if !locked {
            ui.add_space(6.0);
            if self.render_studio.dm26_export.is_some() {
                sui::hint(ui,"Writing DM26 snapshots…");
            } else {
                let available = self.render_studio.timeline.demos.iter().any(|clip| !self.render_studio.timeline.demo_layer_hidden(clip.layer));
                if sui::button_row(ui,&[(Some(Icon::Export),"Export playable demo (.dm_26)",ButtonKind::Normal,available)]).is_some() {
                    self.start_render_studio_dm26_export();
                }
                sui::hint(ui,"Steps the timeline once to record the exact Studio camera, then writes a demo any JKA client can play (demos/studio_*.dm_26). Lights, lens/FOV, transitions and imported audio are not part of a DM26; notes are printed to the console.");
            }
        }
        if !self.render_studio.status.is_empty() {ui.add_space(4.0);sui::hint(ui,&self.render_studio.status);}
    }

    /// Modal frame shared by Studio dialogs: title bar with a close button.
    fn studio_modal<R>(ctx:&egui::Context,id:&str,title:&str,width:f32,add:impl FnOnce(&mut egui::Ui)->R)->(bool,R) {
        let response=egui::Modal::new(egui::Id::new(id))
            .backdrop_color(egui::Color32::from_black_alpha(140))
            .frame(egui::Frame::new().fill(sui::PANE).stroke(egui::Stroke::new(1.0,theme::LINE_STRONG))
                .corner_radius(4).inner_margin(egui::Margin::same(14)))
            .show(ctx,|ui| {
                sui::editor_style(ui);
                ui.set_width(width);
                let mut close=false;
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(title).size(16.0).color(theme::TEXT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui| {
                        close=sui::icon_button(ui,Icon::Close,true,false,"Close (Esc)").clicked();
                    });
                });
                ui.add_space(6.0);
                (close,add(ui))
            });
        let close=response.inner.0 || response.should_close();
        (close,response.inner.1)
    }

    /// Internal egui file browser. No Win32 modal/window-mode transitions:
    /// it stays visible on the same surface in exclusive fullscreen.
    fn egui_render_studio_file_picker(&mut self,ctx:&egui::Context) {
        let Some(kind)=self.render_studio.file_picker.kind else {return};
        let (close,action)=Self::studio_modal(ctx,"studio_import_file_browser",kind.title(),650.0,|ui| {
            self.render_studio.file_picker.show(ui)
        });
        use crate::app::render_studio_picker::StudioPickerAction;
        match if close {Some(StudioPickerAction::Cancel)} else {action} {
            Some(StudioPickerAction::Cancel)=>self.render_studio.file_picker.kind=None,
            Some(StudioPickerAction::Import(kind,files))=>{
                self.render_studio.file_picker.kind=None;
                if !files.is_empty() {
                    match kind {
                        crate::app::render_studio_picker::StudioPickerKind::Audio=>self.studio_queue_audio_files(files),
                        crate::app::render_studio_picker::StudioPickerKind::Demo=>self.studio_import_demo_files(files),
                    }
                }
            }
            None=>{}
        }
    }

    fn egui_render_studio_render_dialog(&mut self,ctx:&egui::Context) {
        if !self.render_studio.show_render {return;}
        let settings_were_open=self.render_studio.graphics_settings_open;
        let (close,_)=Self::studio_modal(ctx,"render_studio_export_panel","Export",480.0,|ui| {
            self.egui_render_studio_output(ui);
        });
        // An export in flight keeps its progress visible until cancelled.
        if close && !self.studio_locked() {self.render_studio.show_render=false;}
        // The modal would block the graphics window opened from its gear.
        if !settings_were_open && self.render_studio.graphics_settings_open {self.render_studio.show_render=false;}
    }

    fn egui_render_studio_model_picker(&mut self,ctx:&egui::Context) {
        let Some(lane)=self.render_studio.model_picker_lane else{return;};
        let mut selected=None::<String>;
        let (close,_)=Self::studio_modal(ctx,"studio_model_picker",&format!("Character {} model",lane+1),440.0,|ui| {
            sui::search_field(ui,&mut self.render_studio.model_search,"Search player models / skins");
            let search=self.render_studio.model_search.trim().to_ascii_lowercase();
            let models:Vec<_>=self.profile_models.iter().filter(|entry|
                    search.is_empty() || entry.value.to_ascii_lowercase().contains(&search)
                    ||entry.model_name.to_ascii_lowercase().contains(&search)
                    ||entry.skin_name.to_ascii_lowercase().contains(&search)).collect();
            sui::hint(ui,&format!("{} models / skins",models.len()));
            let current=self.render_studio.actor_model(lane);
            if sui::list_row(ui,Icon::Layers,theme::TEXT_FAINT,"Project default model","",current.is_empty(),egui::Sense::click()).clicked() {
                selected=Some(String::new());
            }
            ui.separator();
            egui::ScrollArea::vertical().id_salt(("studio_actor_models",lane,&search))
                .max_height(440.0).auto_shrink([false,true]).show_rows(ui,29.0,models.len(),|ui,rows| {
                ui.spacing_mut().item_spacing.y=1.0;
                for row in rows {
                    let entry=models[row];
                    if sui::list_row(ui,Icon::Anim,studio_layer_color(lane),&format!("{} / {}",entry.model_name,entry.skin_name),
                        "",current==entry.value,egui::Sense::click()).clicked() {
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
            self.render_studio.model_picker_lane=None;
        } else if close {self.render_studio.model_picker_lane=None;}
    }

}

impl App {
    /// Hit areas have stable IDs and dragging is always measured against the
    /// pointer's *drag-start* position. Moving a rectangle never feeds back
    /// into its own delta (the old sticky-drag failure).
    fn egui_render_studio_timeline(&mut self, ui: &mut egui::Ui, rect: egui::Rect, header: egui::Rect,
        ruler: egui::Rect, tracks: egui::Rect, scrollbar: egui::Rect, locked: bool) {
        use crate::app::render_studio_timeline::{StudioDrag,StudioDragKind,StudioTrackRow};
        let (ctrl,shift)=ui.ctx().input(|input|(input.modifiers.ctrl||input.modifiers.command,input.modifiers.shift));
        // Lane geometry shared by the fixed ruler, the scrolled lanes and the
        // scrollbar. A vertical scrollbar gutter is always reserved so all
        // three keep the same time-to-x mapping.
        let header_w=sui::TRACK_HEADER_W;
        let w=(tracks.width()-10.0).max(header_w+60.0);
        let lanes_x0=tracks.left()+header_w;
        let lanes_right=tracks.left()+w;
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
            self.render_studio.timeline.zoom_at(x-lanes_x0,wheel);
        }
        sui::paint_pane(ui.painter(),rect);
        self.egui_studio_timeline_header(ui,header,locked,lanes_right-lanes_x0);
        self.egui_studio_timeline_ruler(ui,ruler,lanes_x0,lanes_right,locked);
        let mut delete_lane=None;
        let mut toggle_speed_lane=None;
        let mut toggle_demo_layer:Option<usize>=None;
        // The ruler is its own fixed row; lane geometry starts at the top of
        // the scrolled content.
        let ruler_h=0.0;
        let lane_h=sui::LANE_H;
        let rows=self.render_studio.timeline.track_count();
        // One empty slot below the tracks: dropping media there makes a track.
        let content_h=((rows+1) as f32*lane_h+4.0).max(tracks.height());
        ui.scope_builder(egui::UiBuilder::new().id_salt("studio_timeline").max_rect(tracks), |ui| {
            ui.set_clip_rect(tracks.intersect(ui.clip_rect()));
            egui::ScrollArea::vertical().id_salt("studio_track_scroll").auto_shrink([false,false])
                .max_height(tracks.height()).show(ui,|ui| {
                        let (draw,_)=ui.allocate_exact_size(egui::vec2(w,content_h),egui::Sense::hover());
                        let x0=draw.left()+header_w;
                        let right=draw.right();
                        let zoom=self.render_studio.timeline.zoom;
                        let scroll=self.render_studio.timeline.scroll;
                        let to_x=|t:f64|x0+(t as f32-scroll)*zoom;
                        let from_x=|x:f32|((x-x0)/zoom+scroll).max(0.0) as f64;
                        let painter=ui.painter().with_clip_rect(draw);
                        painter.rect_filled(draw,0.0,sui::WELL);
                        for row in 0..rows {
                            let y=draw.top()+ruler_h+row as f32*lane_h;
                            let kind=self.render_studio.timeline.row_kind(row);
                            let sub_row=matches!(kind,Some(StudioTrackRow::DemoSpeed(_)|StudioTrackRow::DemoAudio(_)|
                                StudioTrackRow::AudioVolume(_)|StudioTrackRow::Motion(_)|StudioTrackRow::Speed(_)|StudioTrackRow::Light(_, _)));
                            let lane_rect=egui::Rect::from_min_max(egui::pos2(x0,y),egui::pos2(right,y+lane_h));
                            let head=egui::Rect::from_min_max(egui::pos2(draw.left(),y),egui::pos2(x0,y+lane_h));
                            painter.rect_filled(lane_rect,0.0,if row%2==0 {sui::LANE_A} else {sui::LANE_B});
                            painter.rect_filled(head,0.0,if sub_row {sui::PANE} else {sui::PANE_HEADER});
                            let actor_lane=if let Some(StudioTrackRow::Character(lane))=kind {Some(lane)}else{None};
                            let actor_row=actor_lane.is_some();
                            let demo_layer=if let Some(StudioTrackRow::Demo(layer))=kind {Some(layer)}else{None};
                            let visible=actor_lane.is_none_or(|lane|self.render_studio.timeline.lane_is_visible(lane))
                                && demo_layer.is_none_or(|layer|!self.render_studio.timeline.demo_layer_hidden(layer));
                            let label=match kind {
                                Some(StudioTrackRow::Camera)=>"Camera".to_owned(),
                                Some(StudioTrackRow::LightLayer(i))=>self.render_studio.timeline.light_layers[i].name.clone(),
                                Some(StudioTrackRow::Light(i,j))=>self.render_studio.timeline.light_layers[i].lights[j].name.clone(),
                                Some(StudioTrackRow::Demo(layer))=>if self.render_studio.timeline.demo_layer_count()>1 {
                                    format!("Demo {}",layer+1)} else {"Demo POV".to_owned()},
                                Some(StudioTrackRow::DemoSpeed(_))=>"Speed / time remap".to_owned(),
                                Some(StudioTrackRow::DemoAudio(_))=>"Demo audio (linked)".to_owned(),
                                Some(StudioTrackRow::Audio(i))=>self.render_studio.timeline.audio_tracks[i].name.clone(),
                                Some(StudioTrackRow::AudioVolume(_))=>"Volume".to_owned(),
                                Some(StudioTrackRow::Motion(lane))=>format!("Motion {:02}",lane+1),
                                Some(StudioTrackRow::Speed(_))=>"Speed / time remap".to_owned(),
                                Some(StudioTrackRow::Character(lane))=>{
                                    let model=self.render_studio.timeline.lane_models.get(lane)
                                        .filter(|m|!m.trim().is_empty()).unwrap_or(&self.render_studio.model);
                                    let model=if model.is_empty(){"Choose model"}else{model.as_str()};
                                    let model=model.split('/').next().unwrap_or(model);
                                    format!("{model} · {:02}",lane+1)
                                }
                                None=>String::new(),
                            };
                            let color=match kind {
                                Some(StudioTrackRow::Demo(_)|StudioTrackRow::DemoSpeed(_))=>sui::DEMO,
                                Some(StudioTrackRow::DemoAudio(_)|StudioTrackRow::Audio(_)|StudioTrackRow::AudioVolume(_))=>sui::AUDIO,
                                Some(StudioTrackRow::Character(lane)|StudioTrackRow::Motion(lane))=>studio_layer_color(lane),
                                Some(StudioTrackRow::Speed(_))=>sui::SPEED,
                                Some(StudioTrackRow::Camera)=>sui::CAMERA,
                                Some(StudioTrackRow::LightLayer(_)|StudioTrackRow::Light(_, _))=>super::render_studio_lights::LIGHT,
                                None=>theme::TEXT_DIM,
                            };
                            // Track color tab, indented under its parent for sub-tracks.
                            let tab_x=draw.left()+if sub_row {16.0} else {2.0};
                            painter.rect_filled(egui::Rect::from_min_max(egui::pos2(tab_x,y+4.0),
                                egui::pos2(tab_x+3.0,y+lane_h-4.0)),1.0,
                                if visible {color} else {color.gamma_multiply(0.35)});
                            let text_x=draw.left()+if actor_row || demo_layer.is_some() || matches!(kind,Some(StudioTrackRow::LightLayer(_))) {58.0} else if sub_row {28.0}
                                else if matches!(kind,Some(StudioTrackRow::Audio(_))) {34.0} else {14.0};
                            let text_right=x0-if matches!(kind,Some(StudioTrackRow::Audio(_))) {56.0} else {8.0};
                            painter.with_clip_rect(egui::Rect::from_min_max(egui::pos2(text_x,y),egui::pos2(text_right,y+lane_h)))
                                .text(egui::pos2(text_x,y+lane_h/2.0),egui::Align2::LEFT_CENTER,label,
                                    egui::FontId::proportional(if sub_row {11.0} else {12.0}),
                                    if !visible {theme::TEXT_DISABLED} else if sub_row {theme::TEXT_FAINT} else {theme::TEXT});
                            if let Some(kind)=kind {self.egui_studio_light_header(ui,&painter,head,kind,locked);}
                            // Vector chevrons: the embedded font lacks triangle glyphs.
                            if actor_row || demo_layer.is_some() {
                                let (lane,expanded)=if let Some(lane)=actor_lane {
                                    (Some(lane),self.render_studio.timeline.speed_expanded(lane))
                                } else {(None,self.render_studio.timeline.demo_layer_expanded(demo_layer.unwrap_or(0)))};
                                let center=egui::pos2(draw.left()+18.0,y+lane_h*0.5);
                                let toggle_rect=egui::Rect::from_center_size(center,egui::vec2(22.0,lane_h));
                                let hovered=!locked && ui.rect_contains_pointer(toggle_rect);
                                sui::paint_icon(&painter,center,if expanded {Icon::ChevronDown} else {Icon::ChevronRight},
                                    if hovered {theme::TEXT} else {theme::TEXT_FAINT});
                                if !locked {
                                    let toggle=ui.interact(toggle_rect,
                                        ui.make_persistent_id(("studio_expand_speed",lane,demo_layer)),egui::Sense::click());
                                    if toggle.clicked() {
                                        if let Some(lane)=lane {toggle_speed_lane=Some(lane);}
                                        else {toggle_demo_layer=demo_layer;}
                                    }
                                    toggle.on_hover_cursor(egui::CursorIcon::PointingHand)
                                        .on_hover_text(if demo_layer.is_some() {if expanded {"Collapse linked audio and speed"} else {"Expand linked audio and speed"}} else if expanded {"Collapse Speed subtrack"} else {"Expand Speed subtrack"});
                                }
                            }
                            // The eye toggles only presentation, not clip placement,
                            // so hidden tracks stay editable and retain their keys.
                            if actor_row {
                                let lane=actor_lane.expect("actor row has a lane");
                                let eye_rect=egui::Rect::from_center_size(egui::pos2(draw.left()+42.0,y+lane_h*0.5),
                                    egui::vec2(22.0,22.0));
                                let eye_hovered=!locked && ui.rect_contains_pointer(eye_rect);
                                sui::paint_icon(&painter,eye_rect.center(),if visible {Icon::Eye} else {Icon::EyeOff},
                                    if eye_hovered {theme::TEXT} else if visible {theme::TEXT_DIM} else {theme::TEXT_DISABLED});
                                if !locked {
                                    let eye=ui.interact(eye_rect,ui.make_persistent_id(("studio_lane_eye",lane)),egui::Sense::click());
                                    if eye.clicked() {
                                        self.render_studio.timeline.toggle_lane_visibility(lane);
                                        self.profile_preview_key=None;
                                    }
                                    eye.on_hover_text(if visible {"Hide character layer"}else{"Show character layer"});
                                    let label_rect=egui::Rect::from_min_max(egui::pos2(draw.left()+54.0,y),
                                        egui::pos2(x0,y+lane_h));
                                    let r=ui.interact(label_rect,ui.make_persistent_id(("studio_actor_model",lane)),egui::Sense::click());
                                    if r.hovered() {
                                        painter.rect_stroke(label_rect.shrink2(egui::vec2(2.0,5.0)),3.0,
                                            egui::Stroke::new(1.0,theme::LINE),egui::StrokeKind::Inside);
                                    }
                                    if r.clicked(){self.render_studio.model_picker_lane=Some(lane);}
                                    if r.secondary_clicked(){self.render_studio.context_lane=Some(lane);}
                                    r.context_menu(|ui| { sui::editor_style(ui);
                                        if ui.button("Copy all animations on this track").clicked() {
                                            self.render_studio.copy_animation_lane(lane);ui.close();
                                        }
                                        let can_paste=self.render_studio.clipboard_contains_animations();
                                        if ui.add_enabled(can_paste,egui::Button::new("Paste animations at playhead")).clicked() {
                                            let time=self.render_studio.timeline_time();
                                            if self.render_studio.paste_animations_on_lane(lane,Some(time)) {
                                                self.profile_preview_key=None;
                                            }
                                            ui.close();
                                        }
                                        if ui.add_enabled(can_paste,egui::Button::new("Paste animations at original times (sync)")).clicked() {
                                            if self.render_studio.paste_animations_on_lane(lane,None){self.profile_preview_key=None;}
                                            ui.close();
                                        }
                                        ui.separator();
                                        if ui.button(if self.render_studio.timeline.lane_is_visible(lane) {"Hide layer"} else {"Show layer"}).clicked() {
                                            self.render_studio.timeline.toggle_lane_visibility(lane);
                                            self.profile_preview_key=None;ui.close();
                                        }
                                        if self.render_studio.timeline.lanes>1 {
                                            let count=self.render_studio.timeline.clips.iter().filter(|c|c.lane==lane).count();
                                            ui.menu_button("Delete layer…",|ui| {
                                                ui.small(format!("Also deletes {count} animation clip(s)."));
                                                if ui.button("Confirm delete layer").clicked() {delete_lane=Some(lane);ui.close();}
                                            });
                                        } else {ui.small("At least one character layer is required.");}
                                    });
                                    r.on_hover_cursor(egui::CursorIcon::PointingHand)
                                        .on_hover_text("Click to choose model · right-click to copy/paste animations across character tracks");
                                }
                            }
                            if let Some(layer)=demo_layer {
                                let eye_rect=egui::Rect::from_center_size(egui::pos2(draw.left()+42.0,y+lane_h*0.5),
                                    egui::vec2(22.0,22.0));
                                let eye_hovered=!locked && ui.rect_contains_pointer(eye_rect);
                                sui::paint_icon(&painter,eye_rect.center(),if visible {Icon::Eye} else {Icon::EyeOff},
                                    if eye_hovered {theme::TEXT} else if visible {theme::TEXT_DIM} else {theme::TEXT_DISABLED});
                                if !locked {
                                    let eye=ui.interact(eye_rect,ui.make_persistent_id(("studio_demo_layer_eye",layer)),egui::Sense::click());
                                    if eye.clicked() {
                                        let hidden=&mut self.render_studio.timeline.demo_layer_hidden;
                                        hidden.resize(hidden.len().max(layer+1),false);
                                        hidden[layer]=!hidden[layer];
                                        self.profile_preview_key=None;
                                    }
                                    eye.on_hover_text(if visible {"Hide this demo layer (the top visible layer plays where layers overlap)"}
                                        else {"Show this demo layer"});
                                }
                            }
                            painter.line_segment([egui::pos2(draw.left(),y+lane_h-0.5),egui::pos2(right,y+lane_h-0.5)],
                                egui::Stroke::new(1.0,theme::HAIRLINE));
                        }
                        // The insertion ghost uses the SAME forward-fit logic as
                        // add_clip/add_demo. It never displays a false 0-second
                        // placement when the dragged lane already has footage.
                        if let (Some(asset),Some(pointer))=(self.render_studio.media_drag.as_ref(),
                            ui.ctx().input(|i|i.pointer.latest_pos())) {
                            let row=((pointer.y-draw.top()-ruler_h)/lane_h).floor() as isize;
                            let is_demo=asset.demo_game.is_some();
                            let new_track=row>=rows as isize;
                            let demo_layer_target=if !is_demo {None} else if new_track {
                                Some(self.render_studio.timeline.demo_layer_for_drop())
                            } else if row>=0 {self.render_studio.timeline.demo_layer_for_row(row as usize)} else {None};
                            let target_lane=if is_demo || new_track || row<0 {None}
                                else {self.render_studio.timeline.lane_for_row(row as usize)};
                            if pointer.x>=x0 && pointer.x<=right &&
                                (demo_layer_target.is_some() || (!is_demo && (target_lane.is_some() || new_track))) {
                                let target_row=if new_track {rows} else if let Some(layer)=demo_layer_target {
                                    self.render_studio.timeline.demo_row(layer)} else {
                                    self.render_studio.timeline.character_row(target_lane.unwrap_or(0))};
                                let y=draw.top()+ruler_h+target_row as f32*lane_h;
                                let track=egui::Rect::from_min_size(egui::pos2(x0,y),egui::vec2(right-x0,lane_h));
                                painter.rect_stroke(track.shrink(1.0),3.0,egui::Stroke::new(2.0,theme::ACCENT),egui::StrokeKind::Inside);
                                if new_track {
                                    let head=egui::Rect::from_min_max(egui::pos2(draw.left(),y),egui::pos2(x0,y+lane_h));
                                    painter.rect_filled(head,0.0,sui::PANE_HEADER);
                                    painter.text(head.left_center()+egui::vec2(14.0,0.0),egui::Align2::LEFT_CENTER,
                                        if is_demo {"+ New demo layer"} else {"+ New character track"},
                                        egui::FontId::proportional(11.5),theme::ACCENT);
                                }
                                let duration=if let Some(duration)=self.render_studio.media_drag_duration {
                                    Some(duration)
                                }else if is_demo {
                                    self.demo_metadata_cache.get(&asset.name).map(|m|m.duration_ms as f64/1000.0)
                                }else if let Some(source)=asset.cinematic.as_ref() {
                                    self.render_studio.cinematics.catalog.as_ref().and_then(|c|c.as_ref().ok())
                                        .and_then(|c|c.clips.iter().find(|c|c.source==*source))
                                        .map(|c|c.frame_count as f64/f64::from(c.fps).max(1.0))
                                }else {
                                    self.asset_preview_player_presenter.as_ref()
                                        .and_then(|p|p.profile_preview_animation_span(&asset.name))
                                        .map(|(first,end,fps)|(end-first) as f64/f64::from(fps).max(1.0))
                                };
                                let desired=from_x(pointer.x);
                                // A new track is empty: no forward-fit.
                                let span=duration.unwrap_or(1.0).max(0.001);
                                let projected=if new_track {desired} else if let Some(layer)=demo_layer_target {
                                    self.render_studio.timeline.demo_drop_start(desired,span,None,layer)
                                } else {self.render_studio.timeline.drop_start(desired,span,target_lane)};
                                let px=to_x(projected);
                                let duration_pixels=(duration.unwrap_or(1.0) as f32*zoom).max(5.0);
                                let ghost=egui::Rect::from_min_size(egui::pos2(px,y+5.0),
                                    egui::vec2(duration_pixels.min((right-px).max(1.0)),lane_h-10.0));
                                if px<=right {
                                    painter.rect_filled(ghost,3.0,theme::ACCENT.gamma_multiply(0.28));
                                    painter.rect_stroke(ghost,3.0,egui::Stroke::new(1.5,theme::ACCENT),egui::StrokeKind::Inside);
                                    painter.line_segment([egui::pos2(px,y),egui::pos2(px,y+lane_h)],
                                        egui::Stroke::new(2.0,theme::ACCENT));
                                }
                                painter.text(pointer+egui::vec2(12.0,-16.0),egui::Align2::LEFT_CENTER,
                                    if duration.is_some() {format!("{} at {:.2}s",asset.name,projected)}
                                    else {format!("{} at ~{:.2}s (duration pending)",asset.name,projected)},
                                    egui::FontId::proportional(11.0),egui::Color32::WHITE);
                            }
                        }
                        let bottom=draw.top()+ruler_h+rows as f32*lane_h;
                        painter.line_segment([egui::pos2(x0,draw.top()),egui::pos2(x0,bottom)],egui::Stroke::new(1.0,theme::LINE));
                        // Grid lines follow the ruler's major ticks; labels live in the ruler.
                        for x in studio_ruler_ticks(scroll,zoom,right-x0).into_iter().map(|(t,_)|to_x(t)) {
                            if x<x0||x>right {continue;}
                            painter.line_segment([egui::pos2(x,draw.top()),egui::pos2(x,bottom)],
                                egui::Stroke::new(1.0,theme::HAIRLINE.gamma_multiply(0.8)));
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
                                    self.render_studio.context_lane=self.render_studio.timeline.lane_for_row(row);
                                    self.render_studio.context_row=Some(row);
                                }
                            }
                            if area.clicked() || area.dragged_by(egui::PointerButton::Primary) {
                                if let Some(p)=point {
                                    self.render_studio.timeline.jump_to(from_x(p.x));
                                    let row=((p.y-draw.top()-ruler_h)/lane_h).floor().max(0.0) as usize;
                                    self.render_studio.context_lane=self.render_studio.timeline.lane_for_row(row);
                                    self.render_studio.timeline.clear_selection();
                                    self.render_studio.selected_background=false;
                                    self.profile_preview_key=None;
                                }
                            }
                            area.context_menu(|ui| { sui::editor_style(ui);
                                let time=self.render_studio.context_time.unwrap_or(self.render_studio.timeline_time());
                                let row=self.render_studio.context_row;
                                self.egui_studio_track_key_items(ui,row,time);
                                ui.menu_button("Add track",|ui| {sui::editor_style(ui);self.egui_studio_add_track_items(ui);});
                                ui.separator();
                                if ui.add_enabled(self.render_studio.clipboard.is_some(),egui::Button::new("Paste at cursor  (Ctrl+V)")).clicked(){
                                    // Paste at the pointer time, preserving the selected destination row.
                                    if self.render_studio.paste_selection_on_lane_at(self.render_studio.context_lane,time){self.profile_preview_key=None;}
                                    ui.close();
                                }
                                if let Some(lane)=self.render_studio.context_lane {
                                    if ui.add_enabled(self.render_studio.clipboard_contains_animations(),
                                        egui::Button::new(format!("Paste animation(s) on character {} at original times",lane+1))).clicked(){
                                        if self.render_studio.paste_animations_on_lane(lane,None){self.profile_preview_key=None;}
                                        ui.close();
                                    }
                                }
                                if ui.button("Set export IN here").clicked(){self.render_studio.timeline.set_in(time);ui.close();}
                                if ui.button("Set export OUT here").clicked(){self.render_studio.timeline.set_out(time);ui.close();}
                            });
                        }
                        // Stable index-based IDs. Clips are *never* sorted during
                        // a drag, and pointer displacement is never accumulated.
                        let clip_data:Vec<_>=self.render_studio.timeline.clips.iter().enumerate()
                            .map(|(i,c)|(i,c.start,c.end(),c.label().to_owned(),c.lane,c.first_frame,c.end_frame,c.source_fps)).collect();
                        for (i,start,end,label,lane,first_frame,end_frame,fps) in clip_data {
                            if i>=self.render_studio.timeline.clips.len() {continue;}
                            let x1=to_x(start).max(x0);let x2=to_x(end).min(right);
                            if x2<=x1 {continue;}
                            let top=draw.top()+ruler_h+self.render_studio.timeline.character_row(lane) as f32*lane_h+4.0;
                            let block=egui::Rect::from_min_max(egui::pos2(x1,top),egui::pos2(x2,top+lane_h-8.0));
                            let selected=self.render_studio.timeline.is_selected(StudioItem::Clip(i));
                            // Tiny clips retain a 28px context-menu target, without
                            // changing the actual painted duration.
                            let hit=egui::Rect::from_center_size(block.center(),
                                egui::vec2(block.width().max(28.0),block.height()));
                            studio_paint_clip(&painter,block,studio_layer_color(lane),selected,&label,
                                &format!("{first_frame}–{end_frame}"));
                            // The trim handles are separate controls, registered
                            // after the body so they win the edge hit test.
                            if !locked {
                                let r=ui.interact(hit,ui.make_persistent_id(("studio_clip",i)),egui::Sense::click_and_drag());
                                if r.secondary_clicked() {
                                    self.render_studio.context_lane=Some(lane);
                                    self.render_studio.context_time=r.interact_pointer_pos().map(|p|from_x(p.x));
                                    if !self.render_studio.timeline.is_selected(StudioItem::Clip(i)) {
                                        self.render_studio.timeline.select_item(StudioItem::Clip(i),false,false);
                                    }
                                    self.render_studio.selected_background=false;
                                }
                                if r.clicked() {
                                    self.render_studio.context_lane=Some(lane);
                                    self.render_studio.selected_background=false;
                                    self.render_studio.timeline.select_item(StudioItem::Clip(i),ctrl,shift);
                                    if !ctrl && !shift {self.render_studio.timeline.jump_to(start);}
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
                                // Match the demo editor: indicate a horizontal resize over
                                // either trim edge even when the clip body wins hover.
                                let near_edge=ui.ctx().pointer_hover_pos().is_some_and(|p|
                                    block.contains(p) && ((p.x-to_x(start)).abs()<=11.0 || (p.x-to_x(end)).abs()<=11.0));
                                let r=if near_edge {r.on_hover_cursor(egui::CursorIcon::ResizeHorizontal)} else {r};
                                let count_before_menu=self.render_studio.timeline.clips.len();
                                r.context_menu(|ui| { sui::editor_style(ui);
                                    if ui.button("Split at playhead  (K)").clicked(){
                                        let t=self.render_studio.timeline_time();
                                        self.render_studio.timeline.split_clip(i,t);ui.close();
                                        self.profile_preview_key=None;
                                    }
                                    if ui.button("Duplicate after clip").clicked(){
                                        self.render_studio.timeline.duplicate_clip(i);ui.close();
                                    }
                                    if ui.button("Auto-direct this clip").on_hover_text("Director: generate a cinematic shot sequence over this clip").clicked(){
                                        let items=self.render_studio.timeline.selected_items();
                                        let mut clips:Vec<usize>=items.iter().filter_map(|item|match item {StudioItem::Clip(c)=>Some(*c),_=>None}).collect();
                                        if !clips.contains(&i) {clips=vec![i];}
                                        self.studio_direct_stage_clips(clips);ui.close();
                                    }
                                    if ui.button("Smooth with neighbors").on_hover_text("Cross-fade with the clips touching this one (0.3 s each side)").clicked(){
                                        self.studio_smooth_clip_transitions(i,0.3);ui.close();
                                    }
                                    if ui.button("Drop to floor").on_hover_text("Stand the actor on the BSP / demo floor at every motion key").clicked(){
                                        let items=self.render_studio.timeline.selected_items();
                                        let mut clips:Vec<usize>=items.iter().filter_map(|item|match item {StudioItem::Clip(c)=>Some(*c),_=>None}).collect();
                                        if !clips.contains(&i) {clips=vec![i];}
                                        self.studio_drop_clips_to_floor(clips);ui.close();
                                    }
                                    let key_time=self.render_studio.context_time.unwrap_or(self.render_studio.timeline_time()).clamp(start,end);
                                    if ui.button("Add motion key here").clicked(){
                                        self.studio_add_motion_key_at(i,key_time);ui.close();
                                    }
                                    if ui.button("Add speed key here").clicked(){
                                        self.studio_add_clip_speed_key_at(i,key_time);ui.close();
                                    }
                                    if ui.button("Add camera key here").clicked(){
                                        self.capture_studio_preview_camera(key_time);ui.close();
                                    }
                                    ui.separator();
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
                                    ui.menu_button("Copy to character…",|ui| {
                                        for target in 0..self.render_studio.timeline.lanes {
                                            if target==lane {continue;}
                                            if ui.button(format!("Character {} · same timeline time",target+1)).clicked() {
                                                self.render_studio.timeline.select_item(StudioItem::Clip(i),false,false);
                                                if self.render_studio.copy_selection() &&
                                                    self.render_studio.paste_animations_on_lane(target,None) {
                                                    self.profile_preview_key=None;
                                                }
                                                ui.close();
                                            }
                                        }
                                    });
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
                                    painter.rect_filled(egui::Rect::from_center_size(egui::pos2(edge_x,block.center().y),egui::vec2(3.0,block.height()-8.0)),1.0,egui::Color32::from_white_alpha(150));
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
                        // Native demo clips occupy their own track, never an animation lane.
                        // This MUST follow timeline start order, not Vec/insertion
                        // order. After dragging clip #0 past clip #1, clip #0
                        // becomes incoming; its transition highlight needs to
                        // paint on top of clip #1's outgoing body.
                        let demo_data:Vec<_>=self.render_studio.timeline.demo_draw_order().into_iter()
                            .map(|i| {
                                let c=&self.render_studio.timeline.demos[i];
                                (i,c.start,c.end(),c.name.clone(),c.in_ms,c.out_ms)
                            }).collect();
                        // Register demo trim grips after ALL demo bodies. A clip's
                        // OUT handle can sit over the next clip's body, so per-
                        // clip registration would let the later body steal it.
                        let mut demo_grips=Vec::new();
                        let demo_count=demo_data.len();
                        for (i,start,end,name,in_ms,out_ms) in demo_data {
                            let x1=to_x(start).max(x0);let x2=to_x(end).min(right);
                            if x2<=x1{continue;}
                            let layer=self.render_studio.timeline.demos[i].layer;
                            let top=draw.top()+ruler_h+self.render_studio.timeline.demo_row(layer) as f32*lane_h+4.0;
                            let block=egui::Rect::from_min_max(egui::pos2(x1,top),egui::pos2(x2,top+lane_h-8.0));
                            let selected=self.render_studio.timeline.demo_selected(i);
                            let composite_count=self.render_studio.timeline.demos[i].race_overlays.iter()
                                .filter(|o|o.enabled).count();
                            let caption=if let Some(race)=&self.render_studio.timeline.demos[i].comparison {format!("Ghost race · {} runners together",race.runners.len())}
                                else if composite_count==0 {"recorded POV".to_owned()}
                                else {format!("recorded POV · {composite_count} composite{}", if composite_count==1{""}else{"s"})};
                            studio_paint_clip(&painter,block,sui::DEMO,selected,&name,&caption);
                            if let Some(transition)=self.render_studio.timeline.demos.get(i) {
                                if transition.transition_in != crate::app::render_studio_timeline::StudioDemoTransition::Cut {
                                    if let Some(prev)=self.render_studio.timeline.demos.iter().enumerate()
                                        .filter(|(j,c)|*j!=i && c.layer==layer && c.start<start && c.end()>start)
                                        .max_by(|a,b|a.1.start.total_cmp(&b.1.start)) {
                                        let overlap=prev.1.end().min(end);
                                        let span=egui::Rect::from_min_max(egui::pos2(x1,block.top()),
                                            egui::pos2(to_x(overlap).min(x2),block.bottom()));
                                        painter.rect_filled(span,2.0,egui::Color32::from_rgba_unmultiplied(245,195,95,105));
                                        painter.line_segment([span.left_top(),span.right_bottom()],egui::Stroke::new(1.2,egui::Color32::from_rgb(255,224,170)));
                                        if span.width()>48.0 {
                                            painter.with_clip_rect(span.shrink(2.0)).text(span.center(),egui::Align2::CENTER_CENTER,
                                                format!("{:.2}s",overlap-start),egui::FontId::proportional(10.0),
                                                egui::Color32::WHITE);
                                        }
                                    }
                                }
                            }
                            if !locked {
                                let hit=egui::Rect::from_center_size(block.center(),egui::vec2(block.width().max(25.0),block.height()));
                                let r=ui.interact(hit,ui.make_persistent_id(("studio_demo_clip",i)),egui::Sense::click_and_drag());
                                if r.secondary_clicked() {
                                    self.render_studio.context_time=r.interact_pointer_pos().map(|p|from_x(p.x));
                                }
                                if r.clicked(){
                                    self.render_studio.timeline.select_demo(i,ctrl,shift);
                                    self.render_studio.audio.selected_demo_audio=None;
                                    self.render_studio.selected_background=false;
                                    if !ctrl && !shift {self.render_studio.timeline.jump_to(start);}
                                }
                                if r.secondary_clicked() && !self.render_studio.timeline.demo_selected(i) {
                                    self.render_studio.timeline.select_demo(i,false,false);
                                }
                                if r.drag_started() {
                                    use crate::app::render_studio_timeline::StudioDemoDragKind;
                                    // Fallback edge detection if the body wins an egui hit.
                                    // Dedicated grips are registered after all bodies below.
                                    // Decide from press origin, not the drag-threshold position.
                                    let press=ui.ctx().input(|input|input.pointer.press_origin())
                                        .or_else(||r.interact_pointer_pos());
                                    if let Some(p)=press {
                                        let left_edge=to_x(start);
                                        let right_edge=to_x(end);
                                        let kind=if block.width()>26.0 && (p.x-left_edge).abs()<=11.0
                                            && (p.x-left_edge).abs()<=(p.x-right_edge).abs() {StudioDemoDragKind::In}
                                            else if block.width()>26.0 && (p.x-right_edge).abs()<=11.0 {StudioDemoDragKind::Out}
                                            else {StudioDemoDragKind::Move};
                                        self.render_studio.timeline.clear_selection();
                                        self.render_studio.timeline.selected_demo=Some(i);
                                        self.render_studio.demo_drag=Some(crate::app::render_studio::StudioDemoDrag{
                                            index:i,kind,pointer_x:p.x,initial_scroll:scroll,start,in_ms,out_ms});
                                    }
                                }
                                r.context_menu(|ui| { sui::editor_style(ui);
                                    let key_time=self.render_studio.context_time.unwrap_or(self.render_studio.timeline_time()).clamp(start,end);
                                    if ui.button("Add speed key here").clicked(){
                                        self.studio_add_demo_speed_key_at(i,key_time);ui.close();
                                    }
                                    if ui.button("Add camera key here").clicked(){
                                        self.capture_studio_preview_camera(key_time);ui.close();
                                    }
                                    if ui.button("Add Director shot here").clicked(){
                                        let (preset,client)=(self.render_studio.director_new_preset,self.render_studio.director_new_client);
                                        if let Some(n)=self.studio_add_director_shot(preset,client,key_time) {
                                            let id=self.render_studio.timeline.director_shots[n].id;
                                            self.studio_select_director(id);
                                        }
                                        ui.close();
                                    }
                                    let targets=if self.render_studio.timeline.demo_selected(i) {
                                        self.render_studio.timeline.selected_demo_indices()} else {vec![i]};
                                    let many=targets.len()>1;
                                    if ui.add_enabled(self.render_studio.director_job.is_none(),egui::Button::new(
                                        if many {format!("Auto-direct {} clips",targets.len())} else {"Auto-direct this clip".to_owned()}))
                                        .on_hover_text("Generate route view: cut this footage into cinematic Director shots").clicked(){
                                        self.studio_start_route_view(targets.clone());ui.close();
                                    }
                                    ui.separator();
                                    if ui.button(if many {format!("Remove {} demo clips",targets.len())} else {"Remove demo clip".to_owned()}).clicked(){
                                        self.render_studio.timeline.remove_demos(targets);
                                        self.profile_preview_key=None;ui.close();
                                    }
                                });
                                let near_edge=block.width()>26.0 && ui.ctx().pointer_hover_pos().is_some_and(|p|
                                    block.contains(p) && ((p.x-to_x(start)).abs()<=11.0 || (p.x-to_x(end)).abs()<=11.0));
                                let r=if near_edge {r.on_hover_cursor(egui::CursorIcon::ResizeHorizontal)} else {r};
                                r.on_hover_text(if near_edge {
                                    "Drag the IN/OUT edge to trim (also during transitions)"
                                }else{
                                    "Drag to move · overlap another demo to set transition duration · drag white edges to trim"
                                });
                                // Draw grips even when the source participates in an overlap.
                                // Interaction is registered in a second pass above all bodies.
                                if block.width()>26.0 {
                                    use crate::app::render_studio_timeline::StudioDemoDragKind;
                                    for (kind,edge_time) in [(StudioDemoDragKind::In,start),(StudioDemoDragKind::Out,end)] {
                                        let edge_x=to_x(edge_time);
                                        // An offscreen edge is not an editable grip at
                                        // the viewport boundary (where x1/x2 are clipped).
                                        if edge_x<x0 || edge_x>right {continue;}
                                        demo_grips.push((i,kind,edge_x,block.center().y,block.height(),start,in_ms,out_ms));
                                    }
                                }
                            }
                        }
                        // Topmost edge hit regions: always active, including in
                        // crossfades and beneath the neighboring demo body.
                        // The body also detects an edge press as a fallback for
                        // egui's frame-to-frame hover handoff.
                        if !locked && self.render_studio.timeline.demos.len()==demo_count {
                            use crate::app::render_studio_timeline::StudioDemoDragKind;
                            for (i,kind,edge_x,center_y,height,start,in_ms,out_ms) in demo_grips {
                                // Paint edge markers on top of ALL clip bodies,
                                // including the incoming overlay that contains
                                // the outgoing clip's OUT edge.
                                painter.rect_filled(egui::Rect::from_center_size(
                                    egui::pos2(edge_x,center_y),egui::vec2(3.0,height-8.0)),
                                    1.0,egui::Color32::from_white_alpha(150));
                                let grip=egui::Rect::from_center_size(
                                    egui::pos2(edge_x,center_y),egui::vec2(16.0,height));
                                let response=ui.interact(grip,
                                    ui.make_persistent_id(("studio_demo_trim",i,kind as u8)),
                                    egui::Sense::click_and_drag())
                                    .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
                                if response.drag_started() {
                                    let press=ui.ctx().input(|input|input.pointer.press_origin())
                                        .or_else(||response.interact_pointer_pos());
                                    if let Some(p)=press {
                                        self.render_studio.timeline.clear_selection();
                                        self.render_studio.timeline.selected_demo=Some(i);
                                        self.render_studio.demo_drag=Some(crate::app::render_studio::StudioDemoDrag{
                                            index:i,kind,pointer_x:p.x,initial_scroll:scroll,start,in_ms,out_ms});
                                    }
                                }
                                response.on_hover_text(format!("{} · source {:.3}s · drag to trim overlap",
                                    if kind==StudioDemoDragKind::In {"IN"}else{"OUT"},
                                    f64::from(if kind==StudioDemoDragKind::In {in_ms}else{out_ms})/1000.0));
                            }
                        }
                        // Expanded clip-owned Speed rows. Every curve is drawn
                        // from the *source* speed function projected through the
                        // inverse source clock into the actual timeline. This
                        // is the same function used by pose/demo playback.
                        use crate::app::render_studio_timeline::{studio_speed_at,studio_speed_span,studio_add_speed_key,StudioSpeedSelection};
                        let mut add_speed_keys=Vec::new();
                        for (i,clip) in self.render_studio.timeline.clips.iter().enumerate() {
                            let Some(row)=self.render_studio.timeline.speed_row(clip.lane) else {continue;};
                            let start=clip.start;let end=clip.end();
                            let source_in=clip.source_in_seconds();let source_out=clip.source_out_seconds();
                            let top=draw.top()+ruler_h+row as f32*lane_h;
                            let x1=to_x(start).max(x0);let x2=to_x(end).min(right);
                            if x2<=x1 {continue;}
                            let speed_rect=egui::Rect::from_min_max(egui::pos2(x1,top+2.0),egui::pos2(x2,top+lane_h-2.0));
                            let painter=painter.with_clip_rect(speed_rect.intersect(painter.clip_rect()));
                            painter.rect_filled(speed_rect,2.0,sui::WELL.lerp_to_gamma(sui::SPEED,0.12));
                            let baseline=studio_speed_curve_y(1.0,top,lane_h);
                            painter.line_segment([egui::pos2(x1,baseline),egui::pos2(x2,baseline)],
                                egui::Stroke::new(0.7,egui::Color32::from_gray(90)));
                            let samples=((x2-x1)/8.0).ceil().clamp(2.0,200.0) as usize;
                            // Accumulate spans between samples: one walk over the keys.
                            let (mut previous,mut elapsed)=(source_in,0.0);
                            let line=(0..=samples).map(|n| {
                                let source=source_in+(source_out-source_in)*n as f64/samples as f64;
                                elapsed+=studio_speed_span(&clip.speed_keys,previous,source);previous=source;
                                let x=to_x(start+elapsed);
                                egui::pos2(x,studio_speed_curve_y(studio_speed_at(&clip.speed_keys,source),top,lane_h))
                            }).collect::<Vec<_>>();
                            painter.add(egui::Shape::line(line,egui::Stroke::new(2.0,sui::SPEED)));
                            if !locked {
                                let response=ui.interact(speed_rect,ui.make_persistent_id(("studio_speed_curve",i)),egui::Sense::click());
                                if response.double_clicked() {
                                    if let Some(pos)=response.interact_pointer_pos(){add_speed_keys.push((false,i,from_x(pos.x)));}
                                }
                                response.on_hover_text("Speed curve: double-click to add a key · drag diamonds to time/rate");
                            }
                        }
                        {
                            for (i,clip) in self.render_studio.timeline.demos.iter().enumerate() {
                                let Some(row)=self.render_studio.timeline.demo_speed_row(clip.layer) else {continue};
                                let start=clip.start;let end=clip.end();
                                let source_in=f64::from(clip.in_ms)/1000.0;
                                let source_out=f64::from(clip.out_ms)/1000.0;
                                let top=draw.top()+ruler_h+row as f32*lane_h;
                                let x1=to_x(start).max(x0);let x2=to_x(end).min(right);
                                if x2<=x1 {continue;}
                                let speed_rect=egui::Rect::from_min_max(egui::pos2(x1,top+2.0),egui::pos2(x2,top+lane_h-2.0));
                                let painter=painter.with_clip_rect(speed_rect.intersect(painter.clip_rect()));
                                painter.rect_filled(speed_rect,2.0,sui::WELL.lerp_to_gamma(sui::SPEED,0.12));
                                let baseline=studio_speed_curve_y(1.0,top,lane_h);
                                painter.line_segment([egui::pos2(x1,baseline),egui::pos2(x2,baseline)],
                                    egui::Stroke::new(0.7,egui::Color32::from_gray(90)));
                                let samples=((x2-x1)/8.0).ceil().clamp(2.0,200.0) as usize;
                                // Accumulate spans between samples: one walk over the keys.
                                let (mut previous,mut elapsed)=(source_in,0.0);
                                let line=(0..=samples).map(|n| {
                                    let source=source_in+(source_out-source_in)*n as f64/samples as f64;
                                    elapsed+=studio_speed_span(&clip.speed_keys,previous,source);previous=source;
                                    let x=to_x(start+elapsed);
                                    egui::pos2(x,studio_speed_curve_y(studio_speed_at(&clip.speed_keys,source),top,lane_h))
                                }).collect::<Vec<_>>();
                                painter.add(egui::Shape::line(line,egui::Stroke::new(2.0,sui::SPEED)));
                                if !locked {
                                    let response=ui.interact(speed_rect,ui.make_persistent_id(("studio_demo_speed_curve",i)),egui::Sense::click());
                                    if response.double_clicked() {
                                        if let Some(pos)=response.interact_pointer_pos(){add_speed_keys.push((true,i,from_x(pos.x)));}
                                    }
                                    response.on_hover_text("Demo speed: double-click to add a key · drag diamonds to time/rate");
                                }
                            }
                        }
                        for (demo,i,time) in add_speed_keys {
                            if demo {
                                if let Some(clip)=self.render_studio.timeline.demos.get_mut(i) {
                                    let source=clip.source_time_ms(time).max(0) as f64/1000.0;
                                    let rate=studio_speed_at(&clip.speed_keys,source);
                                    studio_add_speed_key(&mut clip.speed_keys,source,rate);
                                    if let Some(k)=clip.speed_keys.iter().position(|k|(k.time-source).abs()<0.00001) {
                                        self.render_studio.timeline.select_speed_key(StudioSpeedSelection::Demo(i,k));
                                    }
                                }
                            } else if let Some(clip)=self.render_studio.timeline.clips.get_mut(i) {
                                let source=clip.source_key_time(time);
                                let rate=studio_speed_at(&clip.speed_keys,source);
                                studio_add_speed_key(&mut clip.speed_keys,source,rate);
                                if let Some(k)=clip.speed_keys.iter().position(|k|(k.time-source).abs()<0.00001) {
                                    self.render_studio.timeline.select_speed_key(StudioSpeedSelection::Animation(i,k));
                                }
                            }
                            self.profile_preview_key=None;
                        }
                        // Clip-owned SPEED keyframes. Unlike the former decorative
                        // vertical ticks, these are proper selectable timeline
                        // diamonds with per-key Inspector editing and Delete.
                        // Keep speed on its source clip rather than a global
                        // speed track: animation and demo clocks are independent.
                        let mut speed_markers=Vec::new();
                        for (i,c) in self.render_studio.timeline.clips.iter().enumerate() {
                            let row=self.render_studio.timeline.character_row(c.lane);
                            let top=draw.top()+ruler_h+row as f32*lane_h+4.0;
                            let block=egui::Rect::from_min_max(egui::pos2(to_x(c.start).max(x0),top),
                                egui::pos2(to_x(c.end()).min(right),top+lane_h-8.0));
                            if block.width()<24.0 {continue;}
                            for (k,key) in c.speed_keys.iter().enumerate() {
                                if key.time<c.source_in_seconds()-0.000001 || key.time>c.source_out_seconds()+0.000001 {continue;}
                                let time=c.timeline_at_source(key.time);
                                let x=to_x(time).clamp(block.left()+10.0,block.right()-10.0);
                                let y=if let Some(speed_row)=self.render_studio.timeline.speed_row(c.lane) {
                                    studio_speed_curve_y(key.rate,draw.top()+ruler_h+speed_row as f32*lane_h,lane_h)
                                } else {block.bottom()-7.0};
                                speed_markers.push((StudioSpeedSelection::Animation(i,k),x,y,time,key.time,key.rate));
                            }
                        }
                        for (i,c) in self.render_studio.timeline.demos.iter().enumerate() {
                            let top=draw.top()+ruler_h+self.render_studio.timeline.demo_row(c.layer) as f32*lane_h+4.0;
                            let block=egui::Rect::from_min_max(egui::pos2(to_x(c.start).max(x0),top),
                                egui::pos2(to_x(c.end()).min(right),top+lane_h-8.0));
                            if block.width()<24.0 {continue;}
                            for (k,key) in c.speed_keys.iter().enumerate() {
                                if key.time<f64::from(c.in_ms)/1000.0-0.000001 || key.time>f64::from(c.out_ms)/1000.0+0.000001 {continue;}
                                let time=c.start+crate::app::render_studio_timeline::studio_speed_span(
                                    &c.speed_keys,f64::from(c.in_ms)/1000.0,key.time);
                                let x=to_x(time).clamp(block.left()+10.0,block.right()-10.0);
                                let y=if let Some(speed_row)=self.render_studio.timeline.demo_speed_row(c.layer) {
                                    studio_speed_curve_y(key.rate,draw.top()+ruler_h+speed_row as f32*lane_h,lane_h)
                                } else {block.bottom()-7.0};
                                speed_markers.push((StudioSpeedSelection::Demo(i,k),x,y,time,key.time,key.rate));
                            }
                        }
                        let mut remove_speed=None;
                        for (speed,x,y,time,source,rate) in speed_markers {
                            let selected=self.render_studio.timeline.selected_speed==Some(speed);
                            let center=egui::pos2(x,y);
                            let fill=if selected {egui::Color32::from_rgb(255,218,130)}
                                else {egui::Color32::from_rgb(246,152,231)};
                            if selected {painter.circle_stroke(center,10.0,egui::Stroke::new(2.0,egui::Color32::WHITE));}
                            painter.add(egui::Shape::convex_polygon(vec![
                                center+egui::vec2(0.0,-6.5),center+egui::vec2(6.5,0.0),
                                center+egui::vec2(0.0,6.5),center+egui::vec2(-6.5,0.0)],
                                fill,egui::Stroke::new(1.0,egui::Color32::from_rgb(35,20,44))));
                            if !locked {
                                let r=ui.interact(egui::Rect::from_center_size(center,egui::vec2(17.0,17.0)),
                                    ui.make_persistent_id(("studio_speed_key",speed)),egui::Sense::click_and_drag());
                                if r.clicked() {
                                    self.render_studio.timeline.select_speed_key(speed);
                                    self.render_studio.selected_background=false;
                                    if !ctrl&&!shift {self.render_studio.timeline.seek(time);}
                                    self.profile_preview_key=None;
                                }
                                if r.drag_started() {
                                    let (keys,source_in,source_out)=match speed {
                                        StudioSpeedSelection::Animation(i,_)=>{
                                            let c=&self.render_studio.timeline.clips[i];
                                            (c.speed_keys.clone(),c.source_in_seconds(),c.source_out_seconds())
                                        }
                                        StudioSpeedSelection::Demo(i,_)=>{
                                            let c=&self.render_studio.timeline.demos[i];
                                            (c.speed_keys.clone(),f64::from(c.in_ms)/1000.0,f64::from(c.out_ms)/1000.0)
                                        }
                                    };
                                    self.render_studio.timeline.select_speed_key(speed);
                                    self.render_studio.timeline.speed_drag=Some(crate::app::render_studio_timeline::StudioSpeedDrag{
                                        key:speed,pointer_x:r.interact_pointer_pos().map_or(x,|p|p.x),
                                        pointer_y:r.interact_pointer_pos().map_or(y,|p|p.y),scroll,
                                        keys,source_in,source_out,source_time:source,rate,
                                    });
                                }
                                r.context_menu(|ui| { sui::editor_style(ui);
                                    if ui.button("Edit speed key").clicked() {
                                        self.render_studio.timeline.select_speed_key(speed);ui.close();
                                    }
                                    if ui.button("Delete speed key").clicked() {remove_speed=Some(speed);ui.close();}
                                });
                                r.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(format!("Speed key: {:.2}x | source {:.3}s | timeline {:.3}s | click to select; Del removes",
                                    rate,source,time));
                            }
                        }
                        if let Some(drag)=self.render_studio.timeline.speed_drag.clone() {
                            let (down,pos)=ui.ctx().input(|i|(i.pointer.primary_down(),i.pointer.interact_pos()));
                            if !down {self.render_studio.timeline.speed_drag=None;}
                            else if let Some(pos)=pos {
                                let dx=f64::from((pos.x-drag.pointer_x)/zoom
                                    + self.render_studio.timeline.scroll-drag.scroll);
                                if self.render_studio.timeline.move_speed_key(&drag,dx,pos.y-drag.pointer_y) {
                                    self.profile_preview_key=None;
                                }
                                ui.ctx().request_repaint();
                            }
                        }
                        if let Some(key)=remove_speed {
                            self.render_studio.timeline.select_speed_key(key);
                            if self.render_studio.timeline.delete_selected_speed_key() {
                                self.profile_preview_key=None;
                            } else {self.render_studio.status="Removing the speed key would overlap another clip".into();}
                        }
                        // The green DM26 audio lane is a linked, editable envelope,
                        // not just a clone of the violet demo clip. Native events
                        // have no PCM waveform; draw the actual authored gain
                        // function and editable fade lengths instead.
                        if self.render_studio.timeline.has_demo_track() {
                            let audio_clips:Vec<_>=self.render_studio.timeline.demo_draw_order().into_iter()
                                .map(|i|(i,self.render_studio.timeline.demos[i].clone())).collect();
                            // Collect interactive details until ALL clip bodies
                            // have been registered. A later overlapping clip's
                            // body must never steal an earlier clip's fade grip.
                            let mut fade_grips=Vec::new();
                            let mut volume_grips=Vec::new();
                            for (i,clip) in audio_clips {
                                let Some(row)=self.render_studio.timeline.demo_audio_row(clip.layer) else {continue};
                                let duration=clip.duration();
                                let clip_end=clip.start+duration;
                                let l=to_x(clip.start).max(x0);
                                let r=to_x(clip_end).min(right);
                                if r<=l {continue;}
                                let y=draw.top()+ruler_h+row as f32*lane_h;
                                let rect=egui::Rect::from_min_max(egui::pos2(l,y+3.0),
                                    egui::pos2(r,y+lane_h-4.0));
                                let muted=clip.audio.mute;
                                let selected=self.render_studio.timeline.selected_demo==Some(i);
                                painter.rect_filled(rect,3.0,if muted {sui::clip_body(theme::TEXT_DISABLED,false)}
                                    else {sui::clip_body(sui::AUDIO,selected)});
                                if selected {
                                    painter.rect_stroke(rect,3.0,egui::Stroke::new(1.5,
                                        egui::Color32::WHITE),egui::StrokeKind::Inside);
                                }
                                // Use a monotonic gain display: 0 at the bottom,
                                // 1x at mid-height, 4x near the top. Unlike a
                                // synthesized waveform, this is exactly the
                                // clip's volume automation + output fades.
                                let gain_y=|g:f32| {
                                    let v=g.clamp(0.0,4.0);
                                    rect.bottom()-3.0 - v/(1.0+v)*(rect.height()-6.0)
                                };
                                let curve=painter.with_clip_rect(rect.shrink(2.0));
                                if rect.width()>3.0 {
                                    let steps=((rect.width()/4.0).ceil() as usize).clamp(2,512);
                                    let last_at=(clip_end-0.00001).max(clip.start);
                                    let mut previous=None;
                                    for step in 0..=steps {
                                        let x=l+(r-l)*step as f32/steps as f32;
                                        let at=from_x(x).clamp(clip.start,last_at);
                                        let p=egui::pos2(x,gain_y(clip.audio_gain_within(at,duration)));
                                        if let Some(last)=previous {
                                            curve.line_segment([last,p],egui::Stroke::new(1.8,
                                                egui::Color32::from_rgb(171,252,205)));
                                        }
                                        previous=Some(p);
                                    }
                                }
                                // Keep fade lengths readable even at 0 seconds:
                                // a small handle at each upper corner can always
                                // be grabbed to create a new fade.
                                for (is_in,length) in [(true,clip.audio.fade_in),(false,clip.audio.fade_out)] {
                                    let edge_time=if is_in {clip.start+length} else {clip_end-length};
                                    let x=to_x(edge_time);
                                    if length>0.00001 {
                                        let a=egui::pos2(if is_in {to_x(clip.start)}else{x},
                                            if is_in {rect.bottom()-2.0}else{rect.top()+2.0});
                                        let b=egui::pos2(if is_in {x}else{to_x(clip_end)},
                                            if is_in {rect.top()+2.0}else{rect.bottom()-2.0});
                                        curve.line_segment([a,b],egui::Stroke::new(1.4,
                                            egui::Color32::from_rgb(253,215,126)));
                                    }
                                    if x>=x0-7.0 && x<=right+7.0 {
                                        fade_grips.push((i,is_in,x,rect.top()+4.0,length,duration));
                                    }
                                }
                                // Labels belong at the top, away from the gain
                                // curve and its volume key handles.
                                painter.with_clip_rect(rect.shrink(2.0)).text(
                                    rect.left_top()+egui::vec2(6.0,2.0),egui::Align2::LEFT_TOP,
                                    format!("{} · event audio · {:.0}%{}",clip.name,
                                        clip.audio.gain*100.0,if muted {" (muted)"} else {""}),
                                    egui::FontId::proportional(10.0),egui::Color32::from_rgb(209,247,229));
                                if !locked {
                                    let hit=ui.interact(rect,ui.make_persistent_id(("studio_demo_linked_audio",i)),
                                        egui::Sense::click());
                                    if hit.clicked() {
                                        self.render_studio.timeline.clear_selection();
                                        self.render_studio.timeline.selected_demo=Some(i);
                                        self.render_studio.audio.selected=None;
                                        self.render_studio.audio.selected_track=None;
                                        self.render_studio.selected_background=false;
                                    }
                                    if hit.secondary_clicked() {
                                        self.render_studio.context_time=hit.interact_pointer_pos().map(|p|from_x(p.x));
                                    }
                                    hit.context_menu(|ui| { sui::editor_style(ui);
                                        if ui.button("Add volume key here").clicked() {
                                            let at=self.render_studio.context_time.unwrap_or(self.render_studio.timeline_time());
                                            self.studio_add_demo_volume_key_at(i,at);
                                            ui.close();
                                        }
                                        if ui.button("Reset audio fades").clicked() {
                                            self.render_studio.timeline.demos[i].audio.fade_in=0.0;
                                            self.render_studio.timeline.demos[i].audio.fade_out=0.0;
                                            ui.close();
                                        }
                                    });
                                    hit.on_hover_text("Linked DM26 game events (no PCM waveform). Drag the gold top-edge handles to fade; edit the gain curve with volume keys.");
                                }
                                for (k,key) in clip.audio.keys.iter().enumerate() {
                                    if key.time<f64::from(clip.in_ms)/1000.0 ||
                                        key.time>f64::from(clip.out_ms)/1000.0 {continue;}
                                    let time=clip.timeline_at_source_ms((key.time*1000.0).round() as i32);
                                    let x=to_x(time);
                                    if x<l-8.0 || x>r+8.0 {continue;}
                                    let center=egui::pos2(x,gain_y(clip.audio_gain_within(time,duration)));
                                    volume_grips.push((i,k,center,time,key.time,key.gain,rect));
                                }
                            }
                            // Draw volume diamonds and register their hit boxes
                            // above every linked-audio clip body.
                            for (i,k,center,time,source,gain,rect) in volume_grips {
                                painter.add(egui::Shape::convex_polygon(vec![
                                    center+egui::vec2(0.0,-5.5),center+egui::vec2(5.5,0.0),
                                    center+egui::vec2(0.0,5.5),center+egui::vec2(-5.5,0.0)],
                                    egui::Color32::from_rgb(255,216,119),
                                    egui::Stroke::new(1.0,egui::Color32::BLACK)));
                                if locked {continue;}
                                let response=ui.interact(egui::Rect::from_center_size(center,egui::vec2(13.0,15.0)),
                                    ui.make_persistent_id(("studio_demo_volume_key",i,k)),
                                    egui::Sense::click_and_drag());
                                if response.clicked() || response.drag_started() {
                                    self.render_studio.timeline.clear_selection();
                                    self.render_studio.timeline.selected_demo=Some(i);
                                    self.render_studio.audio.selected_demo_audio=Some(i);
                                    self.render_studio.audio.selected=None;
                                    self.render_studio.selected_background=false;
                                }
                                if response.drag_started() {
                                    if let Some(p)=response.interact_pointer_pos() {
                                        self.render_studio.audio.demo_gain_drag=Some((i,k,p.x,p.y,source,gain));
                                    }
                                }
                                if let Some((t,key,px,py,original_time,original_gain))=self.render_studio.audio.demo_gain_drag {
                                    if t==i && key==k && response.dragged() {
                                        if let Some(p)=response.interact_pointer_pos() {
                                            let shift=f64::from((p.x-px)/zoom);
                                            // Source seconds follow the demo speed
                                            // curve, not a linear output clock.
                                            let original_t=self.render_studio.timeline.demos[i]
                                                .timeline_at_source_ms((original_time*1000.0).round() as i32);
                                            let clip=&self.render_studio.timeline.demos[i];
                                            let desired=(original_t+shift).clamp(clip.start,clip.end());
                                            let source=f64::from(clip.source_time_ms(desired))/1000.0;
                                            let source_duration=f64::from(self.render_studio.timeline.demos[i].duration_ms)/1000.0;
                                            let audio=&mut self.render_studio.timeline.demos[i].audio;
                                            let lo=if k==0 {0.0} else {audio.keys[k-1].time+0.001};
                                            let hi=if k+1==audio.keys.len() {
                                                source_duration
                                            }else{audio.keys[k+1].time-0.001};
                                            if hi>=lo {audio.keys[k].time=source.clamp(lo,hi);}
                                            audio.keys[k].gain=(original_gain+(py-p.y)/(rect.height()-6.0)*2.0)
                                                .clamp(0.0,4.0);
                                        }
                                    }
                                }
                                response.on_hover_cursor(egui::CursorIcon::Grab)
                                    .on_hover_text(format!("Volume {:.0}% · source {:.3}s · output {:.3}s · drag X to retime, Y to adjust volume",gain*100.0,source,time));
                            }
                            // Fade handles are registered last, so a fade endpoint
                            // still wins when two linked audio rectangles overlap.
                            for (i,is_in,x,y,length,duration) in fade_grips {
                                let center=egui::pos2(x,y);
                                painter.circle_filled(center,4.0,egui::Color32::from_rgb(255,222,135));
                                painter.circle_stroke(center,4.0,egui::Stroke::new(1.0,
                                    egui::Color32::from_rgb(80,53,29)));
                                if locked {continue;}
                                let grip=ui.interact(egui::Rect::from_center_size(center,egui::vec2(13.0,14.0)),
                                    ui.make_persistent_id(("studio_demo_audio_fade",i,is_in)),
                                    egui::Sense::click_and_drag());
                                if grip.drag_started() {
                                    if let Some(p)=grip.interact_pointer_pos() {
                                        self.render_studio.audio.demo_fade_drag=Some((i,is_in,p.x,length));
                                        self.render_studio.timeline.clear_selection();
                                        self.render_studio.timeline.selected_demo=Some(i);
                                        self.render_studio.audio.selected_demo_audio=Some(i);
                                        self.render_studio.audio.selected=None;
                                    }
                                }
                                if let Some((t,side,px,original))=self.render_studio.audio.demo_fade_drag {
                                    if t==i && side==is_in && grip.dragged() {
                                        if let Some(p)=grip.interact_pointer_pos() {
                                            let delta=f64::from((p.x-px)/zoom)*if is_in {1.0}else{-1.0};
                                            let audio=&mut self.render_studio.timeline.demos[i].audio;
                                            if is_in {audio.fade_in=(original+delta).clamp(0.0,duration);}
                                            else {audio.fade_out=(original+delta).clamp(0.0,duration);}
                                        }
                                    }
                                }
                                grip.on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
                                    .on_hover_text(format!("{} {:.3}s · drag to adjust linked demo fade",if is_in {"Fade in"} else {"Fade out"},length));
                            }
                            // A released pointer must clear drag state even if
                            // its original grip moved offscreen while dragging.
                            if !ui.ctx().input(|i|i.pointer.primary_down()) {
                                self.render_studio.audio.demo_fade_drag=None;
                                self.render_studio.audio.demo_gain_drag=None;
                            }
                        }
                        self.egui_studio_audio_clips(ui,draw,ruler_h,lane_h,x0,right,zoom,scroll,locked);
                        // Director live shots are clips on the Camera row, under its keys.
                        self.egui_studio_light_timeline(ui,&painter,draw,x0,right,ruler_h,lane_h,zoom,scroll,locked);
                        let camera_row=egui::Rect::from_min_size(egui::pos2(x0,draw.top()+ruler_h),egui::vec2(right-x0,lane_h));
                        self.egui_studio_director_spans(ui,&painter,camera_row,x0,right,zoom,scroll,locked);
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
                                if selected {egui::Color32::from_rgb(255,214,140)} else {sui::CAMERA},egui::Stroke::new(1.0,if selected {egui::Color32::WHITE} else {sui::WELL})));
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
                                r.context_menu(|ui| { sui::editor_style(ui);
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
                                let local=key.time;
                                (local>=c.source_in_seconds()-0.00001 && local<=c.source_out_seconds()+0.00001)
                                    .then_some((i,k,c.timeline_at_source(local)))
                            })).collect();
                        for (number,(i,k,time)) in keys.into_iter().enumerate() {
                            let x=to_x(time);if x<x0-10.0||x>right+10.0 {continue;}
                            let lane=self.render_studio.timeline.clips[i].lane;
                            let y=draw.top()+ruler_h+(self.render_studio.timeline.motion_row(lane) as f32+0.5)*lane_h;
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
                                        let before=if k==0 {c.source_in_seconds()}else{c.motion[k-1].time+1.0/c.source_fps};
                                        let after=if k+1>=c.motion.len(){c.source_out_seconds()}else{c.motion[k+1].time-1.0/c.source_fps};
                                        let sampled=c.source_key_time(d.initial_time+f64::from((p.x-d.pointer_x)/zoom));
                                        c.motion[k].time=sampled.clamp(before,after.max(before));
                                        self.render_studio.timeline.selected_motion=Some((i,k));
                                        self.profile_preview_key=None;
                                    }
                                }
                                if r.drag_stopped(){self.render_studio.timeline.drag=None;}
                                let mut deleted=false;
                                r.context_menu(|ui| { sui::editor_style(ui);
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
                                        painter.rect_filled(popup,4.0,sui::PANE);
                                        painter.text(popup.center(),egui::Align2::CENTER_CENTER,
                                            format!("Frame {frame} · {t:.3}s"),egui::FontId::proportional(11.0),egui::Color32::WHITE);
                                    }
                                }
                            }
                        }
                        // Capture remains stable even when a trimmed edge moves
                        // outside its original egui hit rectangle or the timeline pans.
                        if let Some(d)=self.render_studio.demo_drag {
                            let (down,pos)=ui.ctx().input(|i|(i.pointer.primary_down(),i.pointer.interact_pos()));
                            if !down {self.render_studio.demo_drag=None;}
                            else if let Some(pos)=pos {
                                let seconds=f64::from((pos.x-d.pointer_x)/zoom+
                                    self.render_studio.timeline.scroll-d.initial_scroll);
                                use crate::app::render_studio_timeline::StudioDemoDragKind;
                                match d.kind {
                                    StudioDemoDragKind::Move => {
                                        // Dragging over another demo layer (or below the tracks)
                                        // moves the clip there.
                                        let row=((pos.y-draw.top()-ruler_h)/lane_h).floor() as isize;
                                        let layer=if row>=rows as isize {Some(self.render_studio.timeline.demo_layer_for_drop())}
                                            else if row>=0 {self.render_studio.timeline.demo_layer_for_row(row as usize)} else {None};
                                        match layer {
                                            Some(layer)=>self.render_studio.timeline.move_demo_to_layer(d.index,layer,d.start+seconds),
                                            None=>self.render_studio.timeline.move_demo(d.index,d.start+seconds),
                                        }
                                    }
                                    StudioDemoDragKind::In|StudioDemoDragKind::Out =>
                                        self.render_studio.timeline.trim_demo(d.index,d.kind,d.start,d.in_ms,d.out_ms,seconds),
                                }
                            }
                        }
                        let head_x=to_x(self.render_studio.timeline_time());
                        if head_x>=x0&&head_x<=right {
                            painter.line_segment([egui::pos2(head_x,draw.top()),egui::pos2(head_x,draw.bottom())],
                                egui::Stroke::new(1.5,sui::PLAYHEAD));
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
                                        let target=if row>=0 {
                                            self.render_studio.timeline.lane_for_row(row as usize)
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
                                                let offset=c.source_in_seconds();
                                                let k=d.sub_index;
                                                let before=if k==0 {offset}else{c.motion[k-1].time+1.0/c.source_fps};
                                                let after=if k+1>=c.motion.len(){c.source_out_seconds()}else{c.motion[k+1].time-1.0/c.source_fps};
                                                let sampled=c.source_key_time(desired);
                                                c.motion[k].time=sampled.clamp(before,after.max(before));
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
                                    let inside=ui.clip_rect().contains(p) && p.x>=x0 && p.x<=right;
                                    let new_track=row>=rows as isize;
                                    let lane=if row>=0 {self.render_studio.timeline.lane_for_row(row as usize)}else{None};
                                    let layer=if new_track {Some(self.render_studio.timeline.demo_layer_for_drop())}
                                        else if row>=0 {self.render_studio.timeline.demo_layer_for_row(row as usize)} else {None};
                                    let group=std::mem::take(&mut self.render_studio.media_drag_group);
                                    if inside && asset.demo_game.is_some() && layer.is_some() {
                                        let layer=layer.unwrap_or(0);
                                        self.render_studio_queue_demo_drop(asset.name,asset.demo_game.unwrap(),from_x(p.x),layer);
                                        // The rest follow back to back once each duration is known.
                                        for item in group {
                                            if let Some(game)=item.demo_game {self.render_studio.pending_demo_queue.push_back((item.name,game,layer));}
                                        }
                                    } else if inside && asset.demo_game.is_none() && (new_track || lane.is_some()) {
                                        // The empty slot below the tracks creates the track.
                                        let lane=if new_track {self.render_studio.timeline.lane_for_drop()} else {lane};
                                        match lane {
                                            Some(lane)=>{
                                                self.render_studio_drop_media(asset,from_x(p.x),lane);
                                                for item in group {
                                                    let at=self.render_studio.timeline.clips.iter().filter(|c|c.lane==lane)
                                                        .map(|c|c.end()).fold(from_x(p.x),f64::max);
                                                    self.render_studio_drop_media(item,at,lane);
                                                }
                                            }
                                            None=>self.render_studio.status="All 12 character tracks are in use".into(),
                                        }
                                    } else if inside {
                                        self.render_studio.status="Drop animations onto a character track and demos onto a demo layer, or below the tracks for a new one".into();
                                    }
                                }
                            }
                        }
                    });
        });
        self.egui_studio_timeline_scrollbar(ui,scrollbar,lanes_x0,lanes_right);
        if let Some(lane)=toggle_speed_lane {self.render_studio.timeline.toggle_speed_track(lane);}
        if let Some(layer)=toggle_demo_layer {self.render_studio.timeline.toggle_demo_layer(layer);}
        if let Some(lane)=delete_lane {
            if self.render_studio.timeline.remove_lane(lane) {
                self.render_studio.model_picker_lane=None;
                self.render_studio.status=format!("Deleted character layer {} and its clips",lane+1);
                self.profile_preview_key=None;
            }
        }
    }

    /// Timeline tool strip: track tools on the left, zoom on the right.
    fn egui_studio_timeline_header(&mut self,ui:&mut egui::Ui,header:egui::Rect,locked:bool,track_w:f32) {
        use crate::app::render_studio_timeline::StudioInterpolation;
        let tools=sui::pane_header(ui.painter(),header,"TIMELINE");
        let zoom_w=262.0_f32.min(tools.width()*0.45);
        let split=tools.right()-zoom_w;
        let left=egui::Rect::from_min_max(tools.min,egui::pos2(split-8.0,tools.bottom()));
        let right=egui::Rect::from_min_max(egui::pos2(split,tools.top()),tools.max);
        sui::region(ui,left,"studio_timeline_tools",egui::Layout::left_to_right(egui::Align::Center),|ui| {
            ui.spacing_mut().item_spacing.x=4.0;
            let add=sui::button(ui,Some(Icon::Plus),"Track",ButtonKind::Normal,!locked)
                .on_hover_text("Add a character track, demo layer or audio track (also: right-click the timeline)");
            egui::Popup::menu(&add).style(sui::menu_style()).show(|ui| {
                sui::editor_style(ui);
                self.egui_studio_add_track_items(ui);
            });
            self.egui_studio_audio_menu(ui,locked);
            self.egui_studio_director_menu(ui,locked);
            if sui::button(ui,Some(Icon::AddKey),"Motion key",ButtonKind::Normal,!locked)
                .on_hover_text("Add a character motion key at the playhead on the selected (or current) clip").clicked() {
                let t=self.render_studio.timeline_time();
                let i=self.render_studio.timeline.selected_clip.or_else(||self.render_studio.timeline.clips.iter()
                    .position(|c|t>=c.start&&t<=c.end()));
                if let Some(i)=i {
                    let smooth=self.render_studio.timeline.interpolation!=StudioInterpolation::Linear;
                    let clip=&mut self.render_studio.timeline.clips[i];
                    let mut key=clip.motion_at(t,smooth);key.time=clip.source_key_time(t);
                    let k=clip.add_motion_key(key);
                    self.render_studio.timeline.select_item(StudioItem::Motion(i,k),false,false);
                }
            }
            ui.add_space(10.0);
            let selected=self.render_studio.timeline.selected_items().len();
            if selected>1 {
                ui.label(egui::RichText::new(format!("{selected} selected")).size(11.5).color(theme::ACCENT));
            }
            if !self.render_studio.status.is_empty() {
                let status=self.render_studio.status.clone();
                ui.add(egui::Label::new(egui::RichText::new(&status).size(11.5).color(theme::TEXT_FAINT)).truncate())
                    .on_hover_text(status);
            } else {
                ui.add(egui::Label::new(egui::RichText::new(format!("{} clips · {} demos · {} audio tracks · {} camera keys",
                    self.render_studio.timeline.clips.len(),self.render_studio.timeline.demos.len(),
                    self.render_studio.timeline.audio_tracks.len(),self.render_studio.timeline.camera.len()))
                    .size(11.5).color(theme::TEXT_FAINT)).truncate());
            }
        });
        sui::region(ui,right,"studio_timeline_zoom",egui::Layout::right_to_left(egui::Align::Center),|ui| {
            ui.spacing_mut().item_spacing.x=2.0;
            let now=self.render_studio.timeline_time() as f32;
            let timeline=&mut self.render_studio.timeline;
            if sui::button(ui,None,"Fit",ButtonKind::Ghost,true).on_hover_text("Fit all footage into the visible timeline").clicked() {
                let duration=timeline.duration().max(1.0);
                timeline.zoom=((track_w.max(100.0) as f64)/duration) as f32;
                timeline.zoom=timeline.zoom.clamp(0.1,350.0);
                timeline.scroll=0.0;
            }
            if sui::icon_button(ui,Icon::ZoomIn,true,false,"Zoom in").clicked() {timeline.zoom=(timeline.zoom*1.25).min(350.0);}
            ui.spacing_mut().slider_width=(ui.available_width()-64.0).clamp(40.0,110.0);
            ui.add(egui::Slider::new(&mut timeline.zoom,0.1..=350.0).logarithmic(true).show_value(false))
                .on_hover_text("0.1–350 pixels/second · Alt+wheel over the timeline zooms at the pointer");
            if sui::icon_button(ui,Icon::ZoomOut,true,false,"Zoom out").clicked() {timeline.zoom=(timeline.zoom/1.25).max(0.1);}
            ui.add_space(6.0);
            if sui::icon_button(ui,Icon::Center,true,false,"Center the view on the playhead").clicked() {
                let visible=(track_w/timeline.zoom).max(0.1);
                let max_scroll=(timeline.duration() as f32-visible).max(0.0)+3.0;
                timeline.scroll=(now-visible*0.5).max(0.0).min(max_scroll);
            }
        });
    }

    /// Fixed ruler row: timecode cell over the track headers, ticks, the
    /// export range with draggable IN/OUT brackets and the playhead head.
    fn egui_studio_timeline_ruler(&mut self,ui:&mut egui::Ui,ruler:egui::Rect,x0:f32,right:f32,locked:bool) {
        use crate::app::render_studio_timeline::{StudioDrag,StudioDragKind};
        let zoom=self.render_studio.timeline.zoom.max(0.0001);
        let scroll=self.render_studio.timeline.scroll;
        let to_x=|t:f64|x0+(t as f32-scroll)*zoom;
        let from_x=|x:f32|((x-x0)/zoom+scroll).max(0.0) as f64;
        let now=self.render_studio.timeline_time();
        let head_cell=egui::Rect::from_min_max(ruler.left_top(),egui::pos2(x0,ruler.bottom()));
        let lanes=egui::Rect::from_min_max(egui::pos2(x0,ruler.top()),egui::pos2(right,ruler.bottom()));
        let p=ui.painter().clone();
        p.rect_filled(head_cell,0.0,sui::PANE_HEADER);
        p.rect_filled(lanes,0.0,sui::WELL);
        p.line_segment([ruler.left_bottom(),egui::pos2(right,ruler.bottom())],egui::Stroke::new(1.0,theme::LINE));
        p.line_segment([egui::pos2(x0,ruler.top()),egui::pos2(x0,ruler.bottom())],egui::Stroke::new(1.0,theme::LINE));
        p.text(head_cell.left_center()+egui::vec2(sui::PAD,0.0),egui::Align2::LEFT_CENTER,
            sui::timecode(now,self.render_studio.fps),egui::FontId::monospace(14.0),theme::ACCENT);
        let clip=p.with_clip_rect(lanes);
        let step=studio_ruler_step(zoom);
        let minor=step/if step>=10.0 {5.0} else {4.0};
        let first=(scroll as f64/minor).floor().max(0.0) as i64;
        let last=((scroll as f64+((right-x0)/zoom) as f64)/minor).ceil() as i64;
        for tick in first..=last.min(first+2000) {
            let x=to_x(tick as f64*minor);
            clip.line_segment([egui::pos2(x,ruler.bottom()-4.0),egui::pos2(x,ruler.bottom())],
                egui::Stroke::new(1.0,theme::LINE_STRONG));
        }
        for (t,label) in studio_ruler_ticks(scroll,zoom,right-x0) {
            let x=to_x(t);
            clip.line_segment([egui::pos2(x,ruler.bottom()-10.0),egui::pos2(x,ruler.bottom())],
                egui::Stroke::new(1.0,theme::TEXT_FAINT));
            clip.text(egui::pos2(x+4.0,ruler.top()+4.0),egui::Align2::LEFT_TOP,label,
                egui::FontId::proportional(10.5),theme::TEXT_DIM);
        }
        let (in_time,out_time)=self.render_studio.timeline.export_range();
        let (in_x,out_x)=(to_x(in_time),to_x(out_time));
        let range=egui::Rect::from_min_max(egui::pos2(in_x.max(x0),ruler.bottom()-7.0),
            egui::pos2(out_x.min(right).max(in_x.max(x0)),ruler.bottom()-1.0));
        clip.rect_filled(range,0.0,theme::ACCENT.gamma_multiply(0.35));
        if !locked {
            let r=ui.interact(lanes,ui.make_persistent_id("studio_ruler"),egui::Sense::click_and_drag());
            if let Some(pointer)=r.interact_pointer_pos().filter(|_|r.clicked()||r.dragged()) {
                let near_marker=(pointer.x-in_x).abs()<9.0 || (pointer.x-out_x).abs()<9.0;
                if !near_marker && self.render_studio.timeline.drag.is_none() {
                    self.render_studio.timeline.jump_to(from_x(pointer.x));self.profile_preview_key=None;
                }
            }
            r.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        // IN / OUT brackets sit above ruler scrubbing. Their drag origin
        // stays fixed even while marker geometry moves.
        for (kind,time) in [(StudioDragKind::In,in_time),(StudioDragKind::Out,out_time)] {
            let x=to_x(time);
            if x<x0-10.0||x>right+10.0 {continue;}
            let marker=egui::Rect::from_center_size(egui::pos2(x,ruler.center().y),egui::vec2(14.0,ruler.height()));
            let hovered=!locked && ui.rect_contains_pointer(marker);
            let color=if hovered {theme::TEXT} else {theme::ACCENT};
            let dir=if kind==StudioDragKind::In {1.0} else {-1.0};
            let stroke=egui::Stroke::new(2.0,color);
            clip.line_segment([egui::pos2(x,ruler.top()+3.0),egui::pos2(x,ruler.bottom()-1.0)],stroke);
            clip.line_segment([egui::pos2(x,ruler.top()+3.0),egui::pos2(x+5.0*dir,ruler.top()+3.0)],stroke);
            clip.line_segment([egui::pos2(x,ruler.bottom()-1.0),egui::pos2(x+5.0*dir,ruler.bottom()-1.0)],stroke);
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
                r.on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
                    .on_hover_text(format!("{} {:.3}s · drag to set the export range (I / O set it at the playhead)",
                        if kind==StudioDragKind::In{"IN"}else{"OUT"},time));
            }
        }
        let hx=to_x(now);
        if hx>=x0-6.0 && hx<=right+6.0 {
            clip.add(egui::Shape::convex_polygon(vec![
                egui::pos2(hx-6.0,ruler.top()+3.0),egui::pos2(hx+6.0,ruler.top()+3.0),
                egui::pos2(hx+6.0,ruler.bottom()-10.0),egui::pos2(hx,ruler.bottom()-4.0),
                egui::pos2(hx-6.0,ruler.bottom()-10.0)],sui::PLAYHEAD,egui::Stroke::NONE));
            clip.line_segment([egui::pos2(hx,ruler.bottom()-4.0),egui::pos2(hx,ruler.bottom())],
                egui::Stroke::new(1.5,sui::PLAYHEAD));
        }
    }

    /// Overview scrollbar: the thumb is the visible time window. The export
    /// range and playhead are marked so the whole edit reads at a glance.
    fn egui_studio_timeline_scrollbar(&mut self,ui:&mut egui::Ui,bar:egui::Rect,x0:f32,right:f32) {
        let zoom=self.render_studio.timeline.zoom.max(0.0001);
        let visible=((right-x0)/zoom).max(0.1);
        let duration=self.render_studio.timeline.duration() as f32;
        let max_scroll=(duration-visible).max(0.0)+3.0;
        let total=max_scroll+visible;
        let track=egui::Rect::from_min_max(egui::pos2(x0,bar.top()+3.0),egui::pos2(right,bar.bottom()-3.0));
        let to_bar=|t:f32|track.left()+(t/total).clamp(0.0,1.0)*track.width();
        let thumb_for=|scroll:f32| {
            let left=to_bar(scroll);
            egui::Rect::from_min_max(egui::pos2(left,track.top()),
                egui::pos2(to_bar(scroll+visible).max(left+24.0).min(track.right()),track.bottom()))
        };
        let id=ui.make_persistent_id("studio_timeline_scrollbar");
        let r=ui.interact(track,id,egui::Sense::click_and_drag());
        if let Some(pointer)=r.interact_pointer_pos() {
            if r.drag_started() || r.clicked() {
                if !thumb_for(self.render_studio.timeline.scroll.min(max_scroll)).contains(pointer) {
                    let t=(pointer.x-track.left())/track.width()*total;
                    self.render_studio.timeline.scroll=(t-visible*0.5).clamp(0.0,max_scroll);
                }
                let s0=self.render_studio.timeline.scroll;
                ui.data_mut(|d|d.insert_temp(id,(pointer.x,s0)));
            } else if r.dragged() {
                if let Some((px,s0))=ui.data(|d|d.get_temp::<(f32,f32)>(id)) {
                    self.render_studio.timeline.scroll=(s0+(pointer.x-px)/track.width()*total).clamp(0.0,max_scroll);
                    ui.ctx().request_repaint();
                }
            }
        }
        let p=ui.painter();
        p.rect_filled(track,4.0,sui::WELL);
        p.rect_stroke(track,4.0,egui::Stroke::new(1.0,theme::HAIRLINE),egui::StrokeKind::Inside);
        let (in_time,out_time)=self.render_studio.timeline.export_range();
        p.rect_filled(egui::Rect::from_min_max(egui::pos2(to_bar(in_time as f32),track.top()),
            egui::pos2(to_bar(out_time as f32),track.bottom())),2.0,theme::ACCENT.gamma_multiply(0.18));
        let thumb=thumb_for(self.render_studio.timeline.scroll.min(max_scroll));
        let active=r.hovered() || r.dragged();
        p.rect_filled(thumb,4.0,if active {theme::CONTROL_HOVER} else {theme::CONTROL});
        p.rect_stroke(thumb,4.0,egui::Stroke::new(1.0,if active {theme::LINE_STRONG} else {theme::LINE}),egui::StrokeKind::Inside);
        let hx=to_bar(self.render_studio.timeline_time() as f32);
        p.line_segment([egui::pos2(hx,track.top()),egui::pos2(hx,track.bottom())],egui::Stroke::new(1.5,sui::PLAYHEAD));
        r.on_hover_text("Drag to pan the timeline (the playhead stays put) · Alt+wheel zooms");
    }

    /// Track-wide interpolation, shown with every camera / motion context.
    fn egui_studio_interpolation_section(&mut self, ui:&mut egui::Ui) {
        use crate::app::render_studio_timeline::StudioInterpolation;
        sui::section(ui,"studio_insp_interpolation","Interpolation",
            Some("Applies to the whole timeline.\nLinear: constant velocity.\nSmooth: ease in / out.\nSpline: curved camera path with eased actor motion."),
            true,|ui| {
            sui::segmented(ui,&mut self.render_studio.timeline.interpolation,&[
                (StudioInterpolation::Linear,"Linear"),(StudioInterpolation::Smooth,"Smooth"),(StudioInterpolation::Spline,"Spline")]);
        });
    }

    /// Inspector: a title card for the selection, then collapsible property
    /// sections (Premiere's Effect Controls / Resolve's Inspector).
    fn egui_render_studio_inspector(&mut self, ui:&mut egui::Ui) {
        let fps=self.render_studio.fps;
        if let Some(preview)=self.render_studio.cinematic_preview.as_ref() {
            let clip=&preview.clip;
            sui::inspector_title(ui,theme::WARNING,"Cinematic source",&clip.label,
                &format!("{} frames · {:.2} fps",clip.frame_count,clip.fps));
            sui::section(ui,"studio_insp_cinematic","Source",None,true,|ui| {
                sui::prop_value(ui,"Animation set",&clip.source.gla);
                sui::prop_value(ui,"Animation",&clip.source.animation);
                sui::prop_value(ui,"Frames",&format!("{} – {} (end exclusive)",clip.first_frame,clip.end_frame()));
                sui::prop_value(ui,"Loop frames",&clip.loop_frames.to_string());
            });
            sui::hint(ui,"Add it at the playhead from the transport bar, or drag it from Media onto a character track to trim, extend or repeat it and key motion and cameras.");
            return;
        }
        let locked=self.studio_locked();
        if self.render_studio.hud_editing {
            ui.add_enabled_ui(!locked,|ui|self.egui_studio_hud_inspector(ui));
            return;
        }
        self.studio_director_sync_selection();
        self.studio_sync_light_selection();
        if self.render_studio.light_ui.selection.is_some() {ui.add_enabled_ui(!locked,|ui|{self.egui_studio_light_inspector(ui);});return;}
        if let Some(index)=self.studio_director_index() {
            ui.add_enabled_ui(!locked,|ui| self.egui_studio_director_inspector(ui,index));
            return;
        }
        if (self.render_studio.audio.selected.is_some() || self.render_studio.audio.selected_track.is_some())
            && self.render_studio.timeline.selected_demo.is_none()
            && self.render_studio.timeline.selected_clip.is_none() && !self.render_studio.selected_background {
            ui.add_enabled_ui(!locked,|ui|{self.egui_studio_audio_inspector(ui);});
            return;
        }
        ui.add_enabled_ui(!locked,|ui| {
            let now=self.render_studio.timeline_time();
            if let Some(i)=self.render_studio.timeline.selected_demo {
                if self.render_studio.audio.selected_demo_audio==Some(i) {
                    if let Some(clip)=self.render_studio.timeline.demos.get(i) {
                        sui::inspector_title(ui,sui::AUDIO,"Linked demo audio",&format!("{}.dm_26",clip.name),
                            "follows the demo's position, trims and speed");
                        self.egui_studio_demo_audio_inspector(ui,i,now);
                        if sui::button_row(ui,&[(Some(Icon::Demo),"Edit parent demo footage",ButtonKind::Normal,true)]).is_some() {
                            self.render_studio.audio.selected_demo_audio=None;
                        }
                    }
                    return;
                }
                if let Some(clip)=self.render_studio.timeline.demos.get(i).cloned(){
                    self.egui_studio_demo_inspector(ui,i,clip,now);
                    return;
                }
            }
            if self.render_studio.selected_background {
                sui::inspector_title(ui,sui::SCENE,"Background","Project setting",
                    "behind every character clip on the stage");
                self.egui_studio_background_settings(ui);
                return;
            }
            let selected=self.render_studio.timeline.selected_items();
            if selected.len()>1 {
                sui::inspector_title(ui,theme::ACCENT,"Selection",&format!("{} items",selected.len()),
                    "Ctrl+click toggles · Shift+click extends a time range");
                match sui::button_row(ui,&[(None,"Copy  (Ctrl+C)",ButtonKind::Normal,true),
                    (None,"Delete  (Del)",ButtonKind::Danger,true)]) {
                    Some(0)=>{self.render_studio.copy_selection();}
                    Some(1)=>{self.render_studio.timeline.delete_selection();self.profile_preview_key=None;}
                    _=>{}
                }
                ui.add_space(8.0);
                self.egui_studio_interpolation_section(ui);
                return;
            }
            if let Some(i)=self.render_studio.timeline.selected_camera.filter(|&i|i<self.render_studio.timeline.camera.len()) {
                self.egui_studio_camera_key_inspector(ui,i);
                return;
            }
            if let Some((i,k))=self.render_studio.timeline.selected_motion {
                let number=crate::app::render_studio_spatial::visible_motion_keys(&self.render_studio.timeline)
                    .iter().position(|&(clip,key,_)|clip==i && key==k).map_or(k+1,|n|n+1);
                if let Some(clip)=self.render_studio.timeline.clips.get_mut(i) {
                    if k<clip.motion.len() {
                        let lane=clip.lane;
                        let time=clip.timeline_at_source(clip.motion[k].time);
                        sui::inspector_title(ui,studio_layer_color(lane),&format!("Motion key {number}"),
                            &clip.label().to_owned(),&format!("{} · character {}",sui::timecode(time,fps),lane+1));
                        let key=&mut clip.motion[k];let mut changed=false;
                        let mut delete=false;
                        sui::section(ui,"studio_insp_motion","Transform",
                            Some("Offsets move the character relative to its clip origin. Drag the key in the viewer for direct manipulation."),true,|ui| {
                            changed|=sui::prop_vec3(ui,"Offset",&mut key.position,1.0,["X","Y","Z"],true);
                            changed|=sui::prop_slider(ui,"Actor yaw",egui::Slider::new(&mut key.yaw,-360.0..=360.0).suffix("°")).changed();
                        });
                        if changed{self.profile_preview_key=None;}
                        let mut drop=false;
                        match sui::button_row(ui,&[(None,"Drop to floor",ButtonKind::Normal,true),(None,"Delete motion key",ButtonKind::Danger,true)]) {
                            Some(0)=>drop=true,Some(1)=>delete=true,_=>{}
                        }
                        if drop {self.studio_drop_to_floor(vec![(i,k)]);return;}
                        if delete {
                            clip.motion.remove(k);self.render_studio.timeline.selected_motion=None;
                            self.profile_preview_key=None;
                        }
                        ui.add_space(8.0);
                        self.egui_studio_interpolation_section(ui);
                        self.egui_studio_camera_rig(ui);
                        return;
                    }
                }
            }
            if let Some(i)=self.render_studio.timeline.selected_clip.filter(|&i|i<self.render_studio.timeline.clips.len()) {
                self.egui_studio_clip_inspector(ui,i,now);
                return;
            }
            // Nothing selected: project / playhead context.
            let name=if self.render_studio.project_name.trim().is_empty() {"Untitled project".to_owned()}
                else {self.render_studio.project_name.clone()};
            sui::inspector_title(ui,theme::TEXT_DIM,"Project",&name,
                &format!("playhead {} · {:.2}s long",sui::timecode(now,fps),self.render_studio.timeline.duration()));
            sui::section(ui,"studio_insp_playhead","Playhead",None,true,|ui| {
                sui::prop_value(ui,"Time",&format!("{} ({now:.3} s)",sui::timecode(now,fps)));
                if sui::button_row(ui,&[(Some(Icon::AddKey),"Camera key at playhead",ButtonKind::Normal,true)]).is_some() {
                    self.capture_studio_preview_camera(now);
                }
            });
            self.egui_studio_interpolation_section(ui);
            self.egui_studio_camera_rig(ui);
        });
    }

    fn egui_studio_demo_inspector(&mut self,ui:&mut egui::Ui,i:usize,clip:StudioDemoClip,now:f64) {
        use crate::app::render_studio_timeline::StudioDemoTransition;
        let fps=self.render_studio.fps;
        sui::inspector_title(ui,sui::DEMO,"Demo footage",&format!("{}.dm_26",clip.name),
            &format!("{} · {} – {}",if clip.game.is_empty(){"base"}else{&clip.game},
                sui::timecode(clip.start,fps),sui::timecode(clip.end(),fps)));
        let has_transition=self.render_studio.timeline.demo_has_transition(i);
        let comparison=clip.comparison.is_some();
        if comparison {sui::callout(ui,"One retained ghost race. Camera and playback phases are generated together; regenerate the comparison to change them. HUD edits still apply normally.",theme::ACCENT);}
        sui::section(ui,"studio_insp_demo_clip","Clip",
            Some("Source trims are milliseconds into the recording. While this clip overlaps another (a transition), trims and speed ramps stay locked: move the clips to adjust the overlap instead."),true,|ui| {
            let mut start=clip.start;
            if sui::prop_drag(ui,"Timeline start",egui::DragValue::new(&mut start).speed(0.033).suffix(" s")).changed(){
                self.render_studio.timeline.move_demo(i,start);
            }
            ui.add_enabled_ui(!has_transition&&!comparison,|ui| {
                let previous_end=self.render_studio.timeline.demos.iter().enumerate()
                    .filter(|(j,c)|*j!=i && c.layer==clip.layer && c.end()<=clip.start+0.00001)
                    .map(|(_,c)|c.end()).fold(0.0,f64::max);
                let next_start=self.render_studio.timeline.demos.iter().enumerate()
                    .filter(|(j,c)|*j!=i && c.layer==clip.layer && c.start>=clip.end()-0.00001)
                    .map(|(_,c)|c.start).fold(f64::INFINITY,f64::min);
                let mut in_ms=clip.in_ms;
                if sui::prop_drag(ui,"Source IN",egui::DragValue::new(&mut in_ms)
                    .range(0..=clip.out_ms-1).speed(16.0).suffix(" ms")).changed() {
                    let new_start=clip.timeline_at_source_ms(in_ms);
                    if new_start+0.00001>=previous_end {
                        self.render_studio.timeline.demos[i].in_ms=in_ms;
                        self.render_studio.timeline.demos[i].start=new_start;
                    }else{self.render_studio.status="IN trim would overlap the previous demo".into();}
                }
                let mut out_ms=clip.out_ms;
                if sui::prop_drag(ui,"Source OUT",egui::DragValue::new(&mut out_ms)
                    .range((clip.in_ms+1)..=clip.duration_ms).speed(16.0).suffix(" ms")).changed() {
                    let end=clip.timeline_at_source_ms(out_ms);
                    if end<=next_start+0.00001 {self.render_studio.timeline.demos[i].out_ms=out_ms;}
                    else{self.render_studio.status="OUT trim would overlap the next demo".into();}
                }
            });
            sui::prop_value(ui,"Duration",&format!("{} of {}",format_seconds(clip.duration()),Self::format_demo_time(clip.duration_ms)));
            if has_transition {sui::hint(ui,"Trims and speed are locked while this clip overlaps another.");}
            if comparison {return;}
            let mut hide=clip.hide_other_players;
            if sui::prop_toggle(ui,"Hide other players",&mut hide)
                .on_hover_text("Show only the recorded player (plus a Director shot's subject or look-at target). Other clients' bodies, sabers, shadows and sounds are left out of this clip; projectiles they fired still appear.")
                .changed() {
                let targets=if self.render_studio.timeline.demo_selected(i) {self.render_studio.timeline.selected_demo_indices()} else {vec![i]};
                for j in targets {if let Some(c)=self.render_studio.timeline.demos.get_mut(j) {c.hide_other_players=hide;}}
                self.profile_preview_key=None;
            }
        });
        sui::section(ui,"studio_insp_demo_transition","Transition in",
            Some("Live preview and video export use the same GPU transition compositor. Drag clips on the timeline to change the overlap; the overlap is the transition duration. Hard cut clears it."),true,|ui| {
            let previous=self.render_studio.timeline.demos.iter().enumerate()
                .filter(|(j,c)|*j!=i && c.layer==clip.layer && c.start<clip.start+0.00001)
                .max_by(|a,b|a.1.start.total_cmp(&b.1.start))
                .map(|(_,previous)|(previous.end(),previous.duration()));
            let Some((previous_end,previous_duration))=previous else {
                sui::hint(ui,"Add an earlier demo before creating a transition.");return;
            };
            let maximum=previous_duration.min(clip.duration());
            let mut kind=clip.transition_in;
            let original=kind;
            sui::prop_row(ui,"Type",|ui| {
                sui::combo_box(("studio_demo_transition",i)).width(ui.available_width()).height(420.0)
                    .selected_text(StudioDemoTransition::OPTIONS.iter()
                        .find(|(choice,_)|*choice==kind).map_or("Hard cut",|(_,label)|*label))
                    .show_ui(ui,|ui| {
                        for (index,(choice,label)) in StudioDemoTransition::OPTIONS.into_iter().enumerate() {
                            let group=match index {8=>Some("Depth-aware · scene geometry"),11=>Some("Stylized · screen space"),
                                15=>Some("Geometry · visible entity masks"),_=>None};
                            if let Some(group)=group {
                                ui.separator();
                                sui::group_label(ui,group);
                            }
                            ui.selectable_value(&mut kind,choice,label);
                        }
                    });
            });
            if kind!=original && !self.render_studio.timeline.set_demo_transition_type(i,kind) {
                self.render_studio.status="Overlap clips on the timeline before selecting a transition. A hard cut needs space at the previous clip's end.".into();
            }
            let current_start=self.render_studio.timeline.demos.get(i).map_or(clip.start,|c|c.start);
            let overlap=(previous_end-current_start).max(0.0).min(maximum);
            if overlap>0.00001 {
                sui::prop_value(ui,"Duration",&format!("{overlap:.3} s"));
            } else {
                sui::hint(ui,"Hard cut. Drag this clip over the preceding demo to create a transition; drag farther to lengthen it.");
            }
            if kind.requires_depth() {
                sui::hint(ui,"Uses the real radial depth of both demo worlds (an extra depth prepass per source); sky counts as farthest geometry.");
            } else if matches!(kind, StudioDemoTransition::LumaBrightFirst | StudioDemoTransition::LumaDarkFirst) {
                sui::hint(ui,"Reveals by outgoing image brightness, not scene material IDs.");
            } else if matches!(kind, StudioDemoTransition::Mosaic | StudioDemoTransition::RadialRipple) {
                sui::hint(ui,"Stable screen-space transition; no extra geometry pass.");
            } else if matches!(kind, StudioDemoTransition::PlayerSilhouette | StudioDemoTransition::EntityCascade) {
                sui::hint(ui,"Depth-tested silhouettes of the posed players (then other models for Entity Cascade, then the BSP). Adds a model-mask pass per source while overlapping.");
            }
        });
        ui.add_enabled_ui(!has_transition&&!comparison,|ui| {
            let old_speed=self.render_studio.timeline.demos[i].speed_keys.clone();
            let source_at=f64::from(clip.source_time_ms(now))/1000.0;
            let max_source=f64::from(clip.duration_ms)/1000.0;
            let mut speed_selection=match self.render_studio.timeline.selected_speed {
                Some(crate::app::render_studio_timeline::StudioSpeedSelection::Demo(j,k)) if i==j=>Some(k),
                _=>None,
            };
            let speed_changed=studio_speed_editor(ui,("demo",i),&mut self.render_studio.timeline.demos[i].speed_keys,
                source_at,max_source,now>=clip.start&&now<=clip.end(),&mut speed_selection);
            self.render_studio.timeline.selected_speed=speed_selection.map(|k|
                crate::app::render_studio_timeline::StudioSpeedSelection::Demo(i,k));
            if speed_changed {
                let end=self.render_studio.timeline.demos[i].end();
                let collides=self.render_studio.timeline.demos.iter().enumerate()
                    .any(|(j,c)|j!=i && c.layer==clip.layer && clip.start<c.end()-0.00001 && end>c.start+0.00001);
                if collides || end>36000.0 {
                    self.render_studio.timeline.demos[i].speed_keys=old_speed;
                    self.render_studio.status="Speed change would overlap another demo or exceed 10h".into();
                }else{self.profile_preview_key=None;}
            }
        });
        self.egui_studio_demo_hud_inspector(ui,i);
        self.egui_studio_composite_inspector(ui,i);
        self.egui_studio_demo_audio_inspector(ui,i,now);
        let play_at=now.clamp(clip.start,clip.end());
        match sui::button_row(ui,&[(Some(Icon::AddKey),"Camera key",ButtonKind::Normal,true),
            (Some(Icon::Demo),"Move playhead into clip",ButtonKind::Normal,!(now>=clip.start && now<clip.end()))]) {
            Some(0)=>{
                self.capture_studio_preview_camera(play_at);
                if let Some(i)=self.render_studio.timeline.selected_camera {
                    self.render_studio.timeline.camera[i].source=
                        crate::app::render_studio_timeline::StudioCameraSource::Demo {
                            pov:crate::app::render_studio_timeline::StudioDemoPov::Recorded,client:0};
                }
            }
            Some(1)=>self.render_studio_open_demo_pov(i,play_at),
            _=>{}
        }
        if sui::button_row(ui,&[(None,"Remove clip",ButtonKind::Danger,true)]).is_some() {
            self.render_studio.timeline.remove_demo(i);
            self.render_studio.timeline.selected_demo=None;
        }
    }

    /// Per-source clip override. "Inherit" deliberately stores no entry.
    fn egui_studio_demo_hud_inspector(&mut self,ui:&mut egui::Ui,index:usize) {
        use crate::app::render_studio_cfg::HUD_ELEMENTS;
        let defaults=self.render_studio.hud_settings.clone();
        let targets=self.render_studio.timeline.demo_edit_targets(index);
        if targets.is_empty(){return;}
        sui::section(ui,"studio_demo_hud","HUD",
            Some("Changes apply to all selected demos. Each element inherits the Studio profile unless explicitly overridden. Mixed means the selected demos have different overrides."),true,|ui| {
            let custom=targets.iter().any(|&i|!self.render_studio.timeline.demos[i].hud_overrides.is_empty());
            sui::hint(ui,&format!("{} demo{} · project profile provides defaults",targets.len(),if targets.len()==1{""}else{"s"}));
            if custom && ui.button("Reset all to profile").clicked(){
                self.render_studio.timeline.reset_demo_hud_overrides(index);
                self.profile_preview_key=None;
            }
            for &(key,label) in HUD_ELEMENTS {
                let value=self.render_studio.timeline.demos[index].hud_overrides.get(key).copied();
                let mixed=targets.iter().any(|&i|self.render_studio.timeline.demos[i].hud_overrides.get(key).copied()!=value);
                let mut edit=None;
                sui::prop_row(ui,label,|ui| {
                    egui::ComboBox::from_id_salt(("studio_demo_hud_override",index,key))
                        .selected_text(if mixed {"Mixed"} else {match value {Some(false)=>"Off",Some(true)=>"On",
                            None=>if defaults.enabled(key){"Inherit (On)"}else{"Inherit (Off)"}}})
                        .width(ui.available_width().max(96.0))
                        .show_ui(ui,|ui| {
                            for (choice,label) in [(None,"Inherit"),(Some(false),"Off"),(Some(true),"On")] {
                                if ui.selectable_label(!mixed && value==choice,label).clicked(){edit=Some(choice);}
                            }
                        });
                });
                if let Some(value)=edit {
                    self.render_studio.timeline.set_demo_hud_override(index,key,value);
                    self.profile_preview_key=None;
                }
            }
        });
    }

    fn egui_studio_actor_sabers(&mut self,ui:&mut egui::Ui,lane:usize) {
        let previous=self.render_studio.timeline.lane_sabers.get(lane).cloned().flatten();
        let mut custom=previous.is_some();
        let mut sabers=previous.clone().unwrap_or_else(||self.studio_profile_sabers());
        sui::prop_segmented(ui,"Saber source",&mut custom,&[(false,"Profile"),(true,"Custom")]);
        let mut edited=sui::prop_toggle(ui,"Sabers",&mut sabers.enabled)
            .on_hover_text("Show sabers on this character track. Changes apply to every clip on this track.").changed();
        ui.add_enabled_ui(custom,|ui| {
            for hand in 0..2 {
                let staff=self.profile_sabers.iter().any(|entry|
                    entry.name.eq_ignore_ascii_case(&sabers.names[0]) && Self::profile_saber_is_staff(entry));
                let enabled=hand==0 || !staff;
                ui.add_enabled_ui(enabled,|ui| {
                    sui::prop_row(ui,if hand==0 {"Primary"} else {"Secondary"},|ui| {
                        let current=sabers.names[hand].clone();
                        let title=if current.is_empty() || current.eq_ignore_ascii_case("none") || current.eq_ignore_ascii_case("remove") {
                            "None".to_owned()
                        } else {
                            self.profile_sabers.iter().find(|s|s.name.eq_ignore_ascii_case(&current))
                                .map(|s|Self::profile_saber_label(s).to_owned()).unwrap_or_else(||current.clone())
                        };
                        sui::combo_box(("studio_actor_saber",lane,hand)).width(ui.available_width())
                            .height(300.).wrap_mode(egui::TextWrapMode::Truncate).selected_text(super::jka_colored_text(&title,12.0,theme::TEXT)).show_ui(ui,|ui| {
                                ui.selectable_value(&mut sabers.names[hand],"none".into(),"None");
                                for entry in &self.profile_sabers {
                                    if Self::profile_saber_is_single(entry) || (hand==0 && Self::profile_saber_is_staff(entry)) {
                                        ui.selectable_value(&mut sabers.names[hand],entry.name.clone(),
                                            super::jka_colored_text(Self::profile_saber_label(entry),12.,theme::TEXT));
                                    }
                                }
                            });
                        if sabers.names[hand]!=current {
                            edited=true;
                            let staff=self.profile_sabers.iter().any(|entry|
                                entry.name.eq_ignore_ascii_case(&sabers.names[0]) && Self::profile_saber_is_staff(entry));
                            if staff {sabers.names[1]="none".into();}
                        }
                    });
                    let before=sabers.colors[hand];
                    let packed=crate::net::base_saber_rgb_packed(before as u8);
                    let mut rgb=crate::cgame::custom_saber_rgb(before).unwrap_or([
                        (packed&255) as u8,((packed>>8)&255) as u8,((packed>>16)&255) as u8]);
                    if before==crate::cgame::SABER_BLACK {rgb=[0;3];}
                    sui::prop_row(ui,if hand==0 {"Primary color"}else{"Secondary color"},|ui| {
                        let swatch=egui::Color32::from_rgb(rgb[0],rgb[1],rgb[2]);
                        let foreground=if u32::from(rgb[0])+u32::from(rgb[1])+u32::from(rgb[2])>400 {egui::Color32::BLACK}else{egui::Color32::WHITE};
                        let button=ui.add(egui::Button::new(egui::RichText::new(format!("#{:02X}{:02X}{:02X}",rgb[0],rgb[1],rgb[2])).color(foreground)).fill(swatch));
                        egui::Popup::menu(&button).style(sui::menu_style())
                            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                            let mut chosen=swatch;
                            if egui::color_picker::color_picker_color32(ui,&mut chosen,egui::color_picker::Alpha::Opaque) {
                                rgb=[chosen.r(),chosen.g(),chosen.b()];
                                sabers.colors[hand]=crate::cgame::SABER_CUSTOM_RGB|i32::from(rgb[0])|(i32::from(rgb[1])<<8)|(i32::from(rgb[2])<<16);
                            }
                            ui.separator();ui.label("JKA presets");
                            ui.horizontal_wrapped(|ui| {
                                for (color,name) in [(0,"Red"),(1,"Orange"),(2,"Yellow"),(3,"Green"),(4,"Blue"),(5,"Purple"),(crate::cgame::SABER_BLACK,"Black")] {
                                    if ui.selectable_label(sabers.colors[hand]==color,name).clicked(){sabers.colors[hand]=color;}
                                }
                            });
                        });
                    });
                    edited|=sabers.colors[hand]!=before;
                });
            }
        });
        if custom {sui::hint(ui,"Saved for this character track. Choose a secondary hilt for dual sabers; staff uses the primary slot.");}
        let next=if custom || edited {Some(sabers)} else {None};
        if next!=previous {
            self.render_studio.timeline.lane_sabers.resize(self.render_studio.timeline.lanes,None);
            if let Some(slot)=self.render_studio.timeline.lane_sabers.get_mut(lane) {*slot=next;}
            self.profile_preview_key=None;
        }
    }

    fn egui_studio_clip_inspector(&mut self,ui:&mut egui::Ui,i:usize,now:f64) {
        use crate::app::render_studio_timeline::{StudioInterpolation,StudioClipPlayback};
        let fps=self.render_studio.fps;
        let (label,first,end,source_fps,clip_start,clip_end,source_end,tail,mut lane)= {
            let clip=&self.render_studio.timeline.clips[i];
            (clip.label().to_owned(),clip.first_frame,clip.end_frame,clip.source_fps,
                clip.start,clip.end(),clip.source_end,clip.tail_frames,clip.lane)
        };
        sui::inspector_title(ui,studio_layer_color(lane),"Animation clip",&label,
            &format!("frames {first}–{end} · {source_fps:.2} fps · character {}",lane+1));
        sui::section(ui,"studio_insp_clip","Clip",
            Some("Drag the clip's edges on the timeline to trim non-destructively. Extend adds frames past the source end; After source picks hold or repeat for them."),true,|ui| {
            let mut start=clip_start;
            if sui::prop_drag(ui,"Start",egui::DragValue::new(&mut start).speed(0.033).suffix(" s")).changed(){
                self.render_studio.timeline.move_clip(i,start);self.profile_preview_key=None;
            }
            let layers:Vec<(usize,String)>=(0..self.render_studio.timeline.lanes).map(|l|(l,format!("Character {}",l+1))).collect();
            let layers:Vec<(usize,&str)>=layers.iter().map(|(l,s)|(*l,s.as_str())).collect();
            sui::prop_combo(ui,"Track","studio_inspector_layer",&mut lane,&layers);
            if lane!=self.render_studio.timeline.clips[i].lane {
                self.render_studio.timeline.clips[i].lane=lane;
                let t=self.render_studio.timeline.clips[i].start;
                self.render_studio.timeline.move_clip(i,t);
            }
            let mut extra=tail;
            let next=self.render_studio.timeline.clips.iter().enumerate()
                .filter(|(j,c)|*j!=i && c.lane==lane && c.start>=clip_end-0.00001)
                .map(|(_,c)|c.start).fold(f64::INFINITY,f64::min);
            let max_extra=if next.is_finite() {
                let clip=&self.render_studio.timeline.clips[i];
                let to=crate::app::render_studio_timeline::studio_speed_shift_source(
                    &clip.speed_keys,clip.source_in_seconds(),next-clip_start,
                    clip.source_in_seconds(),f64::from(clip.source_end-clip.source_first+360000)/source_fps);
                ((to*source_fps).floor() as i32-(end-clip.source_first)).max(0)
            } else {360000};
            if sui::prop_drag(ui,"Extend",egui::DragValue::new(&mut extra).range(0..=max_extra.max(0).min(360000))
                .speed(1.0).suffix(" frames")).changed() {
                self.render_studio.timeline.clips[i].tail_frames=extra;
                self.profile_preview_key=None;
            }
            sui::prop_segmented(ui,"After source",&mut self.render_studio.timeline.clips[i].playback,
                &[(StudioClipPlayback::Hold,"Hold last"),(StudioClipPlayback::Loop,"Repeat")]);
            let (clip_start,clip_end)={let c=&self.render_studio.timeline.clips[i];(c.start,c.end())};
            sui::prop_value(ui,"Range",&format!("{} – {}",sui::timecode(clip_start,fps),sui::timecode(clip_end,fps)));
            sui::prop_value(ui,"Source end",&format!("frame {source_end} · {tail} extended"));
        });
        sui::section(ui,"studio_insp_character","Character",
            Some("The model applies to every clip on this character track. Leave it blank to use the project's default model."),true,|ui| {
            if let Some(actor_model)=self.render_studio.timeline.lane_models.get_mut(lane) {
                let mut pick=false;
                sui::prop_row(ui,"Model",|ui| {
                    ui.spacing_mut().item_spacing.x=4.0;
                    ui.add(egui::TextEdit::singleline(actor_model).hint_text("project default")
                        .desired_width((ui.available_width()-30.0).max(40.0)));
                    pick=sui::icon_button(ui,Icon::Search,true,false,"Browse player models").clicked();
                });
                if pick {self.render_studio.model_picker_lane=Some(lane);}
            }
            // Like Model and Visible, this is an actor-track setting, not a
            // change to the live char_color_* userinfo cvars.
            let original=self.render_studio.actor_char_color(lane,self.network.char_color);
            let mut rgb=original.map(|channel|f32::from(channel)/255.0);
            let mut edited=false;
            let mut inherit=false;
            let overridden=self.render_studio.timeline.lane_char_colors.get(lane)
                .copied().flatten().is_some();
            sui::prop_row(ui,"RGB color",|ui| {
                edited=ui.color_edit_button_rgb(&mut rgb)
                    .on_hover_text("JKA char_color_red/green/blue tint for this character track (0–255).")
                    .changed();
                if overridden {
                    inherit=ui.small_button("Profile")
                        .on_hover_text("Use the current Profile color instead of a track override")
                        .clicked();
                }
            });
            if edited || inherit {
                self.render_studio.timeline.lane_char_colors.resize(self.render_studio.timeline.lanes,None);
                self.render_studio.timeline.lane_char_colors[lane]=if inherit {None} else {
                    Some(rgb.map(|channel|(channel.clamp(0.0,1.0)*255.0).round() as u8))
                };
                self.profile_preview_key=None;
            }
            let mut layer_visible=self.render_studio.timeline.lane_is_visible(lane);
            if sui::prop_toggle(ui,"Visible",&mut layer_visible).changed() {
                self.render_studio.timeline.toggle_lane_visibility(lane);
                self.profile_preview_key=None;
            }
            self.egui_studio_actor_sabers(ui,lane);
        });
        let old_speed=self.render_studio.timeline.clips[i].speed_keys.clone();
        let source_at=self.render_studio.timeline.clips[i].source_key_time(now);
        let max_source=f64::from(source_end-self.render_studio.timeline.clips[i].source_first
            +self.render_studio.timeline.clips[i].tail_frames.max(0))/source_fps.max(1.0);
        let mut speed_selection=match self.render_studio.timeline.selected_speed {
            Some(crate::app::render_studio_timeline::StudioSpeedSelection::Animation(j,k)) if i==j=>Some(k),
            _=>None,
        };
        let speed_changed=studio_speed_editor(ui,("clip",i),&mut self.render_studio.timeline.clips[i].speed_keys,
            source_at,max_source,now>=clip_start&&now<=clip_end,&mut speed_selection);
        self.render_studio.timeline.selected_speed=speed_selection.map(|k|
            crate::app::render_studio_timeline::StudioSpeedSelection::Animation(i,k));
        if speed_changed {
            let actor=&self.render_studio.timeline.clips[i];
            let collision=self.render_studio.timeline.clips.iter().enumerate().any(|(j,c)|
                i!=j && c.lane==actor.lane && c.start<actor.end()-0.00001 && c.end()>actor.start+0.00001);
            if collision || actor.end()>36000.0 {
                self.render_studio.timeline.clips[i].speed_keys=old_speed;
                self.render_studio.status="Speed change would overlap another animation or exceed 10h".into();
            }else{self.profile_preview_key=None;}
        }
        let (prev,next)=self.render_studio.timeline.touching_clips(i);
        let mut blend_changed=false;
        sui::section(ui,("studio_insp_clip_blend",i),"Transitions",
            Some("Smooth the cut to the clip touching this one on the same track: pose, position and facing cross-fade over the blend time. When both clips smooth the same cut it becomes one symmetric cross-fade."),
            true,|ui| {
            let max=crate::app::render_studio_timeline::StudioClipBlend::MAX;
            let blend=&mut self.render_studio.timeline.clips[i].blend;
            ui.add_enabled_ui(prev.is_some(),|ui| {
                blend_changed|=sui::prop_slider(ui,"Blend in",egui::Slider::new(&mut blend.blend_in,0.0..=max).suffix(" s"))
                    .on_hover_text("Ease in from the previous clip's last pose").changed();
            });
            ui.add_enabled_ui(next.is_some(),|ui| {
                blend_changed|=sui::prop_slider(ui,"Blend out",egui::Slider::new(&mut blend.blend_out,0.0..=max).suffix(" s"))
                    .on_hover_text("Ease out into the next clip's first pose").changed();
            });
            if prev.is_none() && next.is_none() {
                sui::hint(ui,"Place another clip right before or after this one on the same track to smooth between them.");
            }
        });
        if blend_changed {self.profile_preview_key=None;}
        let selected_clips:Vec<usize>={
            let items=self.render_studio.timeline.selected_items();
            let mut clips:Vec<usize>=items.iter().filter_map(|item|match item {StudioItem::Clip(c)=>Some(*c),_=>None}).collect();
            if !clips.contains(&i) {clips=vec![i];}
            clips
        };
        match sui::button_row(ui,&[(None,"Smooth with neighbors",ButtonKind::Normal,prev.is_some()||next.is_some()),
            (None,if selected_clips.len()>1 {"Drop all to floor"} else {"Drop to floor"},ButtonKind::Normal,true)]) {
            Some(0)=>self.studio_smooth_clip_transitions(i,0.3),
            Some(1)=>self.studio_drop_clips_to_floor(selected_clips),
            _=>{}
        }
        match sui::button_row(ui,&[(Some(Icon::AddKey),"Motion key",ButtonKind::Normal,true),
            (None,"Split  (K)",ButtonKind::Normal,true)]) {
            Some(0)=>{
                let smooth=self.render_studio.timeline.interpolation!=StudioInterpolation::Linear;
                let clip=&mut self.render_studio.timeline.clips[i];
                let mut key=clip.motion_at(now,smooth);
                key.time=clip.source_key_time(now);
                let k=clip.add_motion_key(key);
                self.render_studio.timeline.selected_motion=Some((i,k));
            }
            Some(1)=>{self.render_studio.timeline.split_clip(i,now);self.profile_preview_key=None;}
            _=>{}
        }
        if sui::button_row(ui,&[(None,"Delete clip",ButtonKind::Danger,true)]).is_some() {
            self.render_studio.timeline.clips.remove(i);self.render_studio.timeline.selected_clip=None;
            self.render_studio.timeline.selected_motion=None;self.profile_preview_key=None;
            return;
        }
        ui.add_space(8.0);
        self.egui_studio_interpolation_section(ui);
        self.egui_studio_camera_rig(ui);
    }

    /// Camera key: source first (it decides which controls apply), then the
    /// shot's transform or follow rig, the shared rig, interpolation.
    fn egui_studio_camera_key_inspector(&mut self,ui:&mut egui::Ui,i:usize) {
        let lens_aspect=self.render_studio.demo_viewport.map_or(16.0/9.0,|v|v[2] as f32/v[3].max(1) as f32);
        if let Some(key)=self.render_studio.timeline.camera.get_mut(i) {
            if super::render_studio_camera::studio_target_editor(ui,&mut key.focus_target) {
                key.resolve_focus_target();self.profile_preview_key=None;
            }
        }
        use crate::app::render_studio_timeline::{StudioCameraSource,StudioDemoPov,StudioFollowPerspective,
            StudioFollowFallback,StudioFollowMovePath,StudioFollowMoveAim,StudioDemoAnchor};
        let fps=self.render_studio.fps;
        let has_clips=!self.render_studio.timeline.clips.is_empty();
        let has_demos=!self.render_studio.timeline.demos.is_empty();
        {
            let key=&mut self.render_studio.timeline.camera[i];
            let source_label=match key.source {
                StudioCameraSource::Animation=>"animation stage",
                StudioCameraSource::Demo{pov:StudioDemoPov::Recorded,..}=>"recorded demo POV",
                StudioCameraSource::Demo{pov:StudioDemoPov::Follow,..}=>"follow player",
                StudioCameraSource::Demo{pov:StudioDemoPov::Free,..}=>"free demo camera",
            };
            sui::inspector_title(ui,sui::CAMERA,"Camera key",&format!("Shot {}",i+1),
                &format!("{} · {}",sui::timecode(key.time,fps),source_label));
            let mut changed_source=false;
            sui::section(ui,"studio_insp_camera_source","Source",
                Some("Animation stage: the isolated character stage. Demo recording: the native demo world, with its recorded camera, a follow-player rig or a keyed free camera."),true,|ui| {
                let mut source=key.source;
                let mut is_demo=matches!(source,StudioCameraSource::Demo{..});
                // Only offer the sources the timeline can show; a key on the
                // wrong one stays switchable so it can be fixed.
                let (has_clips,has_demos)=(has_clips,has_demos);
                let mismatch=(is_demo && !has_demos) || (!is_demo && !has_clips && has_demos);
                ui.add_enabled_ui((has_clips && has_demos) || mismatch,|ui| {
                    if sui::segmented(ui,&mut is_demo,&[(false,"Animation stage"),(true,"Demo recording")]) {
                        source=if is_demo {StudioCameraSource::Demo {pov:StudioDemoPov::Follow,client:-1}}
                            else {StudioCameraSource::Animation};
                        if is_demo {key.demo_follow.perspective=StudioFollowPerspective::ThirdPerson;}
                    }
                });
                if mismatch {
                    sui::callout(ui,if is_demo {"No demo footage on the timeline: this key has nothing to show."}
                        else {"The timeline has only demo footage: this key shows an empty stage. Switch it to Demo recording."},theme::WARNING);
                }
                if let StudioCameraSource::Demo{mut pov,client}=source {
                    let before=pov;
                    sui::prop_combo(ui,"Camera",("studio_camera_mode",i),&mut pov,&[
                        (StudioDemoPov::Recorded,"Recorded camera (original POV)"),
                        (StudioDemoPov::Follow,"Follow rig (third person / orbit / eyes)"),
                        (StudioDemoPov::Free,"Free / fixed camera")]);
                    // A new follow rig starts on the recorded player, so it
                    // keeps following the recorder's spectator switches.
                    let client=if pov==StudioDemoPov::Follow && before!=StudioDemoPov::Follow {-1} else {client};
                    source=StudioCameraSource::Demo{pov,client};
                }
                if key.source!=source {key.source=source;changed_source=true;}
            });
            if changed_source {self.profile_preview_key=None;}
        }
        let is_stage=matches!(self.render_studio.timeline.camera[i].source,StudioCameraSource::Animation);
        let following=is_stage && self.render_studio.timeline.camera_rig.position.enabled;
        let tracking=is_stage && self.render_studio.timeline.camera_rig.target.enabled;
        let reference=self.render_studio.timeline.camera_rig.reference_position;
        let demo_players=self.render_studio.sources.iter().find(|source|
            self.render_studio.scene_frame.as_ref().is_some_and(|f|f.outgoing.id==source.id)).map(|source|&source.playback)
            .map(|session|session.demo_visible_players()).unwrap_or_default();
        let preceding_follow=i>0 && matches!(self.render_studio.timeline.camera[i-1].source,
            StudioCameraSource::Demo {pov:StudioDemoPov::Follow,..});
        let preview_camera=self.render_studio.scene_frame.as_ref().map(|scene|
            if scene.progress>=0.5 {scene.incoming.as_ref().unwrap_or(&scene.outgoing).camera} else {scene.outgoing.camera})
            .unwrap_or_else(||self.asset_preview_camera());
        let current_view=(preview_camera.position.to_array(),preview_camera.yaw.to_degrees(),preview_camera.pitch.to_degrees(),preview_camera.cg_fov());
        let mut delete=false;
        let key=&mut self.render_studio.timeline.camera[i];
        if let StudioCameraSource::Demo{pov:StudioDemoPov::Follow,mut client}=key.source {
            let mut changed=false;
            sui::section(ui,"studio_insp_follow_target","Follow target",
                Some("Recorded player follows whoever the demo POV is on at each moment, including the recorder's spectator switches. Pick a specific player to lock onto them instead."),
                true,|ui| {
                Self::studio_client_rows(ui,"studio_follow_clients",&demo_players,&mut client,
                    super::render_studio_director::RECORDED_PLAYER);
            });
            if key.source!=(StudioCameraSource::Demo{pov:StudioDemoPov::Follow,client}) {
                key.source=StudioCameraSource::Demo{pov:StudioDemoPov::Follow,client};
                changed=true;
            }
            let follow=&mut key.demo_follow;
            sui::section(ui,"studio_insp_follow_placement","Placement",
                Some("First person: the actor's eye or bone. Third person: the OpenJK chase camera (its collision and aim, translated to the chosen anchor). Orbit: an authored angle and distance around a body anchor."),true,|ui| {
                sui::segmented(ui,&mut follow.perspective,&[(StudioFollowPerspective::FirstPerson,"First"),
                    (StudioFollowPerspective::ThirdPerson,"Third"),(StudioFollowPerspective::Orbit,"Orbit")]);
                let anchors:Vec<(StudioDemoAnchor,&str)>=StudioDemoAnchor::ALL.iter().map(|a|(*a,a.label())).collect();
                sui::prop_combo(ui,"Anchor",("studio_demo_position_bone",i),&mut follow.position_anchor,&anchors);
                sui::prop_toggle(ui,"Facing-relative",&mut follow.position_local).on_hover_text("Position offset relative to player facing");
                sui::prop_vec3(ui,"Offset",&mut follow.position_offset,0.5,["X","Y","Z"],true);
                if follow.perspective!=StudioFollowPerspective::FirstPerson {
                    sui::prop_slider(ui,"Distance",egui::Slider::new(&mut follow.range,0.0..=2048.0));
                    sui::prop_slider(ui,"Height",egui::Slider::new(&mut follow.vertical_offset,-512.0..=512.0));
                    sui::prop_slider(ui,"Side",egui::Slider::new(&mut follow.horizontal_offset,-512.0..=512.0));
                }
                if follow.perspective==StudioFollowPerspective::Orbit {
                    sui::prop_toggle(ui,"World angles",&mut follow.orbit_world_angles).on_hover_text("Orbit angles in world space (not player-relative)");
                    sui::prop_slider(ui,"Orbit yaw",egui::Slider::new(&mut follow.yaw_offset,-360.0..=360.0).suffix("°"));
                    sui::prop_slider(ui,"Orbit pitch",egui::Slider::new(&mut follow.pitch_offset,-89.0..=89.0).suffix("°"));
                    sui::prop_toggle(ui,"Avoid walls",&mut follow.orbit_collision).on_hover_text("Avoid BSP walls during orbit");
                } else {
                    if follow.perspective==StudioFollowPerspective::ThirdPerson {
                        sui::prop_toggle(ui,"Chase motion",&mut follow.motion_direction).on_hover_text("Chase movement direction (TaystJK angle -1)");
                    }
                    let yaw_free=follow.perspective!=StudioFollowPerspective::ThirdPerson || !follow.motion_direction;
                    ui.add_enabled_ui(yaw_free,|ui| {
                        sui::prop_slider(ui,"Relative yaw",egui::Slider::new(&mut follow.yaw_offset,-180.0..=180.0).suffix("°"));
                    });
                    sui::prop_slider(ui,"Relative pitch",egui::Slider::new(&mut follow.pitch_offset,-89.0..=89.0).suffix("°"));
                }
            });
            sui::section(ui,"studio_insp_follow_aim","Look-at",
                Some("Position and aim anchors are independent. Bone points use the presented Ghoul2 pose when available; missing model or bone data falls back to the eye position."),true,|ui| {
                sui::prop_toggle(ui,"Aim at target",&mut follow.aim_enabled).on_hover_text("Aim the camera at a tracked body point");
                if follow.aim_enabled {
                    Self::studio_client_rows(ui,"studio_aim_clients",&demo_players,&mut follow.aim_client,
                        super::render_studio_director::FOLLOWED_PLAYER);
                    let anchors:Vec<(StudioDemoAnchor,&str)>=StudioDemoAnchor::ALL.iter().map(|a|(*a,a.label())).collect();
                    sui::prop_combo(ui,"Look at",("studio_demo_aim_bone",i),&mut follow.aim_anchor,&anchors);
                    sui::prop_toggle(ui,"Facing-relative",&mut follow.aim_local).on_hover_text("Aim offset relative to target facing");
                    sui::prop_vec3(ui,"Aim offset",&mut follow.aim_offset,0.5,["X","Y","Z"],true);
                } else if follow.perspective==StudioFollowPerspective::Orbit {
                    sui::hint(ui,"Orbit looks at its position anchor. Enable Aim for a different body part or player.");
                } else {
                    sui::hint(ui,"View follows the actor's recorded angles plus the relative yaw / pitch.");
                }
            });
            sui::section(ui,"studio_insp_follow_lens","Lens",None,true,|ui| {
                super::render_studio_camera::studio_lens_editor(ui,"studio_key_lens",&mut key.fov,&mut key.lens,lens_aspect,key.focus_target.is_some());
            });
            let follow=&mut key.demo_follow;
            sui::section(ui,"studio_insp_follow_move","Move from previous shot",
                Some("Moves use both players in one recorded world; arcs are sampled directly from timeline time. Collision clamps each frame toward the subject, not a planned route."),true,|ui| {
                if !preceding_follow {
                    sui::hint(ui,"Add a preceding Follow Player key for a camera move. Other source changes remain cuts.");
                    return;
                }
                sui::prop_slider(ui,"Blend",egui::Slider::new(&mut follow.blend_seconds,0.0..=10.0).suffix(" s"));
                sui::prop_combo(ui,"Path",("studio_move_path",i),&mut follow.move_path,&[
                    (StudioFollowMovePath::Direct,"Straight"),(StudioFollowMovePath::ArcLeft,"Arc left"),
                    (StudioFollowMovePath::ArcRight,"Arc right"),(StudioFollowMovePath::Lift,"Lift / crane")]);
                if follow.move_path!=StudioFollowMovePath::Direct {
                    sui::prop_slider(ui,"Arc offset",egui::Slider::new(&mut follow.move_arc,0.0..=1024.0));
                }
                sui::prop_combo(ui,"Look",("studio_move_aim",i),&mut follow.move_aim,&[
                    (StudioFollowMoveAim::BlendAngles,"Blend view angles"),(StudioFollowMoveAim::LookAtSubjects,"Look at players")]);
                sui::prop_toggle(ui,"BSP clamp",&mut follow.avoid_walls).on_hover_text("BSP collision clamp (experimental)");
            });
            let fallback_help="Used when the target is lost or outside the recorded PVS. Recorded POV stays active if the player or camera is not reconstructible; a missing outgoing player is skipped during a blend. If the backup camera is also outside the recorded PVS, recorded POV wins.";
            sui::section(ui,"studio_insp_follow_fallback","Target lost",Some(fallback_help),false,|ui| {
                sui::prop_combo(ui,"Fallback",("studio_follow_fallback",i),&mut key.demo_follow.fallback,&[
                    (StudioFollowFallback::RecordedPov,"Recorded POV"),(StudioFollowFallback::KeyedCamera,"Keyed backup camera")]);
                if key.demo_follow.fallback==StudioFollowFallback::KeyedCamera {
                    if sui::button_row(ui,&[(Some(Icon::Camera),"Set backup from current view",ButtonKind::Normal,true)]).is_some() {
                        (key.position,key.yaw,key.pitch,key.fov)=current_view;
                    }
                    sui::prop_vec3(ui,"Position",&mut key.position,1.0,["X","Y","Z"],true);
                    sui::prop_slider(ui,"Yaw",egui::Slider::new(&mut key.yaw,-360.0..=360.0).suffix("°"));
                    sui::prop_slider(ui,"Pitch",egui::Slider::new(&mut key.pitch,-89.0..=89.0).suffix("°"));
                    sui::hint(ui,"The backup uses this key's FOV from Lens.");
                }
            });
            if changed {self.profile_preview_key=None;}
            if sui::button_row(ui,&[(None,"Delete camera key  (Del)",ButtonKind::Danger,true)]).is_some() {delete=true;}
        } else {
            if matches!(key.source,StudioCameraSource::Demo {pov:StudioDemoPov::Recorded,..}) {
                sui::hint(ui,"The demo's original camera. To see the recorded player from outside (still following the recorder's spectator switches), use a recorded-player rig:");
                let pick=sui::button_row(ui,&[(Some(Icon::Camera),"Third person",ButtonKind::Normal,true),
                    (None,"Orbit",ButtonKind::Normal,true),(None,"Eyes",ButtonKind::Normal,true)]);
                if let Some(choice)=pick {
                    key.source=StudioCameraSource::Demo {pov:StudioDemoPov::Follow,client:-1};
                    key.demo_follow.perspective=[StudioFollowPerspective::ThirdPerson,
                        StudioFollowPerspective::Orbit,StudioFollowPerspective::FirstPerson][choice];
                    if choice<2 && key.demo_follow.range<40.0 {key.demo_follow.range=120.0;}
                    self.profile_preview_key=None;
                }
            }
            let pose_editable=matches!(key.source,
                StudioCameraSource::Animation|StudioCameraSource::Demo {pov:StudioDemoPov::Free,..});
            let mut changed=false;
            if pose_editable {
                let help=if matches!(key.source,StudioCameraSource::Demo{..}) {
                    "Free camera uses the keyed position, yaw, pitch and FOV; the demo's recorded PVS still limits visible world state."
                } else if following {
                    "Position is relative to the camera rig's follow attachment."
                } else {"Position, orientation and FOV for this shot. Editing a key leaves its neighbors unchanged."};
                sui::section(ui,"studio_insp_camera_transform","Transform",Some(help),true,|ui| {
                    let mut value=std::array::from_fn(|n|key.position[n]-if following {reference[n]}else{0.});
                    if sui::prop_vec3(ui,if following {"Offset"} else {"Position"},&mut value,1.0,["X","Y","Z"],true) {
                        key.position=std::array::from_fn(|n|value[n]+if following {reference[n]}else{0.});
                        changed=true;
                    }
                    ui.add_enabled_ui(!tracking && key.focus_target.is_none(),|ui| {
                        changed|=sui::prop_slider(ui,"Yaw",egui::Slider::new(&mut key.yaw,-360.0..=360.0).suffix("°")).changed();
                        changed|=sui::prop_slider(ui,"Pitch",egui::Slider::new(&mut key.pitch,-89.0..=89.0).suffix("°")).changed();
                    });
                    if tracking {sui::hint(ui,"Aim is controlled by the rig's Track target.");}
                    changed|=super::render_studio_camera::studio_lens_editor(ui,"studio_key_lens",&mut key.fov,&mut key.lens,lens_aspect,key.focus_target.is_some());
                });
            } else {
                sui::prop_value(ui,"35 mm equivalent",&format!("{:.1} mm",crate::renderer::StudioLens::focal_length_35mm(preview_camera.cg_fov(),lens_aspect)));
                changed|=super::render_studio_camera::studio_aperture_editor(ui,&mut key.lens,key.focus_target.is_some());
            }
            if changed {self.profile_preview_key=None;}
            if is_stage {self.egui_studio_camera_rig(ui);}
            self.egui_studio_interpolation_section(ui);
            if sui::button_row(ui,&[(None,"Delete camera key  (Del)",ButtonKind::Danger,true)]).is_some() {delete=true;}
        }
        if delete {
            self.render_studio.timeline.camera.remove(i);
            self.render_studio.timeline.selected_camera=None;
            self.profile_preview_key=None;
        }
    }
}

impl App {
    /// Project names are sanitized by project_file_name; browsing never permits
    /// paths outside the Render Studio write directory.
    /// Graphics profile combo + settings button (viewer header, Export).
    fn egui_studio_profile_picker(&mut self,ui:&mut egui::Ui,locked:bool,width:f32) {
        ui.spacing_mut().item_spacing.x=4.0;
        ui.add_enabled_ui(!locked,|ui| {
            if sui::icon_button(ui,Icon::Settings,true,self.render_studio.graphics_settings_open,
                "Studio graphics settings (separate from your gameplay settings)").clicked() {
                self.render_studio.graphics_settings_open=!self.render_studio.graphics_settings_open;
            }
            // Comparing every renderer cvar is cheap but not free; only check
            // while the graphics window (where edits happen) is open.
            let unsaved=self.render_studio.graphics_settings_open && self.studio_cfg_unsaved();
            let current=self.render_studio.graphics_profile.clone();
            let label=format!("{}{}",current.trim_end_matches(".cfg"),if unsaved {" *"} else {""});
            let mut selected=current.clone();
            sui::combo_box("studio_graphics_profile").width(width)
                .wrap_mode(egui::TextWrapMode::Truncate).selected_text(label)
                .show_ui(ui,|ui| {
                    for name in &self.render_studio.graphics_profiles {
                        ui.selectable_value(&mut selected,name.clone(),name.trim_end_matches(".cfg"));
                    }
                }).response.on_hover_text(if unsaved {
                    "Studio graphics profile · unsaved changes (saved automatically when you switch or leave)"
                } else {"Studio graphics profile: renderer settings used for preview and export"});
            if selected!=current {
                if let Err(error)=self.studio_cfg_select(&selected) {
                    self.render_studio.status=format!("Could not load graphics profile: {error}");
                }
            }
        });
    }

    /// Studio graphics settings: the regular graphics pages, editing the
    /// Studio profile only. Display / window / backend stay gameplay settings.
    fn egui_render_studio_graphics_settings(&mut self,ctx:&egui::Context) {
        if !self.render_studio.graphics_settings_open || self.studio_locked() {return;}
        let mut open=true;
        egui::Window::new("Studio graphics").id(egui::Id::new("studio_graphics_settings"))
            .open(&mut open).default_size([940.0,720.0]).max_width(1100.0).max_height(780.0)
            .default_pos([360.0,70.0]).resizable(true).collapsible(false)
            .frame(egui::Frame::new().fill(sui::PANE).stroke(egui::Stroke::new(1.0,theme::LINE_STRONG))
                .corner_radius(4).inner_margin(egui::Margin::same(12)))
            .show(ctx,|ui| {
                ui.scope(|ui| {
                    sui::editor_style(ui);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Profile").size(12.0).color(theme::TEXT_DIM));
                        self.egui_studio_profile_picker(ui,false,200.0);
                        ui.add_space(8.0);
                        let unsaved=self.studio_cfg_unsaved();
                        if sui::button(ui,Some(Icon::Save),"Save",if unsaved {ButtonKind::Primary} else {ButtonKind::Normal},true)
                            .on_hover_text("Write the current Studio graphics to the selected profile").clicked() {
                            self.render_studio.status=match self.studio_cfg_save() {
                                Ok(())=>format!("Saved {}",self.render_studio.graphics_profile),
                                Err(error)=>format!("Could not save graphics profile: {error}"),
                            };
                        }
                        if sui::button(ui,None,"Match gameplay",ButtonKind::Ghost,true)
                            .on_hover_text("Reset the Studio graphics to your gameplay renderer settings (not saved until you Save)").clicked() {
                            if let Err(error)=self.studio_cfg_match_gameplay() {
                                self.render_studio.status=format!("Could not copy gameplay graphics: {error}");
                            }
                        }
                        ui.add_space(8.0);
                        ui.add(egui::TextEdit::singleline(&mut self.render_studio.graphics_new_name)
                            .desired_width(130.0).hint_text("new profile name"));
                        let name=self.render_studio.graphics_new_name.clone();
                        if sui::button(ui,Some(Icon::Plus),"Duplicate",ButtonKind::Normal,!name.trim().is_empty())
                            .on_hover_text("Copy the current Studio graphics into a new named profile").clicked() {
                            self.render_studio.status=match self.studio_cfg_create(&name) {
                                Ok(())=>{self.render_studio.graphics_new_name.clear();
                                    format!("Created {}",self.render_studio.graphics_profile)}
                                Err(error)=>format!("Could not create graphics profile: {error}"),
                            };
                        }
                        let can_delete=self.render_studio.graphics_profiles.len()>1;
                        let confirm=self.render_studio.graphics_confirm_delete;
                        if sui::button(ui,None,if confirm {"Confirm delete"} else {"Delete"},ButtonKind::Danger,can_delete)
                            .on_hover_text("Delete the selected profile file (gameplay settings are untouched)").clicked() {
                            if confirm {
                                self.render_studio.status=match self.studio_cfg_delete() {
                                    Ok(())=>format!("Deleted profile; now using {}",self.render_studio.graphics_profile),
                                    Err(error)=>format!("Could not delete graphics profile: {error}"),
                                };
                                self.render_studio.graphics_confirm_delete=false;
                            } else {self.render_studio.graphics_confirm_delete=true;}
                        }
                        if confirm && sui::button(ui,None,"Cancel",ButtonKind::Ghost,true).clicked() {
                            self.render_studio.graphics_confirm_delete=false;
                        }
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Page").size(12.0).color(theme::TEXT_DIM));
                        let sections:Vec<(super::VideoSection,&str)>=super::VideoSection::RENDERING.iter()
                            .chain(super::VideoSection::ENVIRONMENT.iter())
                            .chain(super::VideoSection::OPTIMIZATIONS.iter())
                            .chain(super::VideoSection::DEBUG.iter()).copied().collect();
                        ui.allocate_ui(egui::vec2(220.0,24.0),|ui| {
                            sui::combo(ui,"studio_video_section",&mut self.video_section,&sections);
                        });
                        sui::hint(ui,"Edits apply to Studio preview and exports only. Files: render-studio/profiles/*.cfg; the project remembers its profile.");
                    });
                    sui::divider(ui);
                });
                sui::section(ui,"studio_profile_hud","HUD · Studio profile",
                    Some("Independent from gameplay HUD cvars. Saved alongside renderer options in the selected Studio CFG."),true,|ui| {
                    ui.horizontal(|ui| {
                        for preset in ["Clean","Gameplay","Race"] {
                            if ui.small_button(preset).clicked() {
                                self.render_studio.hud_settings=crate::app::render_studio_cfg::StudioHudSettings::preset(preset);
                            }
                        }
                    });
                    for &(key,label) in crate::app::render_studio_cfg::HUD_ELEMENTS {
                        let mut enabled=self.render_studio.hud_settings.enabled(key);
                        if sui::prop_toggle(ui,label,&mut enabled).changed() {
                            self.render_studio.hud_settings.set(key,enabled);
                        }
                    }
                    if ui.button("Edit positions in Program Preview").clicked() {
                        self.render_studio.hud_editing=true;
                        self.align_studio_defaults_for_editor();
                        self.render_studio.graphics_settings_open=false;
                    }
                });
                self.egui_video_page(ui);
            });
        self.render_studio.graphics_settings_open=open;
    }

    fn egui_render_studio_project_dialogs(&mut self, ctx:&egui::Context) {
        if self.render_studio.show_new_project {
            let mut choice=None;
            let (close,_)=Self::studio_modal(ctx,"studio_new_project","New project",480.0,|ui| {
                sui::prop_text(ui,"Name",&mut self.render_studio.new_project_name,"Untitled");
                sui::prop_row(ui,"File",|ui|sui::hint(ui,&self.studio_project_path_for(&self.render_studio.new_project_name).display().to_string()));
                sui::hint(ui,"Start a fresh timeline with default camera and lighting. Save the current project first to keep your edits. Studio graphics and HUD preferences are retained.");
                let available=self.studio_new_project_name_available(&self.render_studio.new_project_name);
                if !available {sui::callout(ui,"Choose an unused project name.",theme::WARNING);}
                if self.render_studio.status.starts_with("Could not save timeline:") {
                    sui::callout(ui,&self.render_studio.status,theme::WARNING);
                }
                ui.add_space(6.0);
                let enabled=available && !self.studio_locked();
                choice=sui::button_row(ui,&[(None,"Cancel",ButtonKind::Normal,true),
                    (Some(Icon::Save),"Save and New",ButtonKind::Normal,enabled),
                    (Some(Icon::Plus),"Create project",ButtonKind::Primary,enabled)]);
            });
            match choice {
                Some(1)|Some(2)=>{
                    let name=self.render_studio.new_project_name.clone();
                    self.new_render_studio_project(name,choice==Some(1));
                }
                Some(_)=>self.render_studio.show_new_project=false,
                None if close=>self.render_studio.show_new_project=false,
                None=>{}
            }
        }
        if self.render_studio.show_save_as {
            if self.render_studio.save_as_name.is_empty() {
                self.render_studio.save_as_name=self.render_studio.project_name.clone();
            }
            let mut save=false;
            let path=self.studio_project_path_for(&self.render_studio.save_as_name);
            let (close,_)=Self::studio_modal(ctx,"studio_save_as","Save project as",420.0,|ui| {
                let r=sui::prop_text(ui,"Name",&mut self.render_studio.save_as_name,"timeline_project");
                if r.lost_focus() && ui.input(|i|i.key_pressed(egui::Key::Enter)) {save=true;}
                sui::prop_row(ui,"File",|ui| sui::hint(ui,&path.display().to_string()));
                if path.exists() {sui::callout(ui,"A project with this name exists; saving overwrites it.",theme::WARNING);}
                ui.add_space(6.0);
                if sui::button_row(ui,&[(Some(Icon::Save),"Save project",ButtonKind::Primary,true)]).is_some() {save=true;}
            });
            if save {
                let current=self.render_studio.project_name.clone();
                self.render_studio.project_name=self.render_studio.save_as_name.clone();
                if self.save_render_studio_project() {
                    self.render_studio.save_as_name.clear();
                    self.render_studio.show_save_as=false;
                } else {self.render_studio.project_name=current;}
            } else if close {
                self.render_studio.save_as_name.clear();
                self.render_studio.show_save_as=false;
            }
        }
        if self.render_studio.show_open_project {
            let mut chosen=None::<String>;
            let dir=self.studio_media_root();
            let mut entries:Vec<_>=std::fs::read_dir(&dir).ok().into_iter().flatten()
                .flatten().filter_map(|e|{
                    let path=e.path();
                    if path.is_dir() && path.join("project.json").is_file() {
                        path.file_name().and_then(|x|x.to_str()).map(str::to_owned)
                    } else {None}
                }).collect();
            entries.sort();
            let (close,_)=Self::studio_modal(ctx,"studio_open_project","Open project",440.0,|ui| {
                sui::hint(ui,&format!("Folder: {}",dir.display()));
                ui.add_space(4.0);
                if entries.is_empty(){sui::callout(ui,"No saved JSON projects found.",theme::TEXT_FAINT);}
                egui::ScrollArea::vertical().max_height(360.0).auto_shrink([false,true]).show(ui,|ui| {
                    ui.spacing_mut().item_spacing.y=1.0;
                    for entry in entries {
                        if sui::list_row(ui,Icon::Open,theme::ACCENT,&entry,"",
                            self.render_studio.project_name==entry,egui::Sense::click()).clicked() {
                            chosen=Some(entry);
                        }
                    }
                });
            });
            if let Some(name)=chosen {
                self.render_studio.project_name=name;
                self.load_render_studio_project();
                self.render_studio.show_open_project=false;
            } else if close {self.render_studio.show_open_project=false;}
        }
    }
}

/// Graph vertical coordinate; logarithmic so 0.5x, 1x, 2x, 4x and 8x
/// remain readable in a compact subtrack.
fn studio_speed_curve_y(rate:f64,top:f32,height:f32)->f32 {
    let norm=(rate.clamp(0.1,8.0)/0.1).ln()/80.0_f64.ln();
    top+height-6.0-(norm as f32)*(height-12.0)
}

/// Clip-local, source-anchored speed automation. Selecting a timeline
/// diamond selects the same key in this Inspector. All edits remain in the
/// existing deterministic source-time integration path.
fn studio_speed_editor(ui:&mut egui::Ui,id:impl std::hash::Hash + std::fmt::Debug,
    keys:&mut Vec<crate::app::render_studio_timeline::StudioSpeedKey>,
    source_at:f64,source_limit:f64,playhead_inside:bool,
    selected:&mut Option<usize>)->bool {
    use crate::app::render_studio_timeline::{StudioSpeedKey,studio_speed_at,studio_add_speed_key};
    let mut changed=false;
    sui::section(ui,("studio_insp_speed",id),"Speed / time remap",
        Some("Keys are source-anchored and speed interpolates linearly between them. Select a pink diamond on the clip (or a key below) to edit it; Delete removes the selected key. Expand the track's chevron to see the curve."),
        true,|ui| {
        let source_at=source_at.clamp(0.0,source_limit);
        if keys.len()<=1 {
            let mut rate=keys.first().map_or(1.0,|k|k.rate);
            if sui::prop_drag(ui,"Clip speed",egui::DragValue::new(&mut rate).range(StudioSpeedKey::MIN..=StudioSpeedKey::MAX)
                .speed(0.05).suffix("×")).changed() {
                keys.clear();keys.push(StudioSpeedKey{time:0.0,rate});
                *selected=Some(0);changed=true;
            }
        } else {sui::prop_value(ui,"At playhead",&format!("{:.2}×",studio_speed_at(keys,source_at)));}
        let mut remove=None;
        for (i,key) in keys.iter_mut().enumerate() {
            ui.push_id(i,|ui| {
                sui::prop_row(ui,&format!("Key {} · {:.2}s",i+1,key.time),|ui| {
                    ui.spacing_mut().item_spacing.x=4.0;
                    let active=*selected==Some(i);
                    if sui::icon_button(ui,Icon::Keys,true,active,"Select this speed key").clicked() {*selected=Some(i);}
                    changed|=ui.add_sized(egui::vec2((ui.available_width()-30.0).max(40.0),20.0),
                        egui::DragValue::new(&mut key.rate).range(StudioSpeedKey::MIN..=StudioSpeedKey::MAX).speed(0.05).suffix("×")).changed();
                    if sui::icon_button(ui,Icon::Close,true,false,"Delete speed key").clicked() {remove=Some(i);}
                });
            });
        }
        if let Some(i)=remove {
            keys.remove(i);changed=true;
            *selected=match *selected {
                Some(k) if k==i=>None,Some(k) if k>i=>Some(k-1),other=>other,
            };
        }
        if let Some(k)=*selected {
            if k<keys.len() {
                let lo=if k==0 {0.0}else{keys[k-1].time+0.001};
                let hi=if k+1<keys.len(){keys[k+1].time-0.001}else{source_limit};
                if hi>=lo {
                    let mut time=keys[k].time;
                    if sui::prop_drag(ui,&format!("Key {} time",k+1),egui::DragValue::new(&mut time).range(lo..=hi)
                        .speed(0.01).suffix(" s")).changed() {
                        keys[k].time=time.clamp(lo,hi);changed=true;
                    }
                }
            } else {*selected=None;}
        }
        match sui::button_row(ui,&[(Some(Icon::AddKey),"Key at playhead",ButtonKind::Normal,playhead_inside&&keys.len()<256),
            (None,"Reset to 1×",ButtonKind::Normal,!keys.is_empty())]) {
            Some(0)=>{
                let rate=studio_speed_at(keys,source_at);
                studio_add_speed_key(keys,source_at,rate);
                *selected=keys.iter().position(|key|(key.time-source_at).abs()<0.00001);
                changed=true;
            }
            Some(1)=>{keys.clear();*selected=None;changed=true;}
            _=>{}
        }
    });
    changed
}

/// Major ruler spacing: the smallest "nice" step at least 56 pt apart.
fn studio_ruler_step(zoom:f32)->f64 {
    [0.5,1.,2.,5.,10.,15.,30.,60.,120.,300.,600.,900.,1800.,3600.]
        .into_iter().find(|s|(*s as f32)*zoom>=56.0).unwrap_or(3600.)
}

/// Labelled major ticks visible in a lane area `width` points wide.
fn studio_ruler_ticks(scroll:f32,zoom:f32,width:f32)->Vec<(f64,String)> {
    let zoom=zoom.max(0.0001);
    let step=studio_ruler_step(zoom);
    let visible=(width/zoom).max(0.0) as f64;
    let first=(scroll as f64/step).floor().max(0.0) as i32;
    let last=((scroll as f64+visible)/step).ceil() as i32;
    (first..=last.min(first+500)).map(|tick| {
        let t=tick as f64*step;
        let label=if t>=3600.0 {format!("{}:{:02}:{:02}",(t/3600.0) as u32,((t/60.0) as u32)%60,(t as u32)%60)}
            else if step>=30.0 {format!("{}:{:02}",(t/60.0) as u32,(t as u32)%60)}
            else {format!("{t:.1}s")};
        (t,label)
    }).collect()
}

/// NLE clip: darkened body, saturated top band, name plus a faint detail.
pub(super) fn studio_paint_clip(p:&egui::Painter,block:egui::Rect,color:egui::Color32,selected:bool,label:&str,detail:&str) {
    p.rect_filled(block,3.0,sui::clip_body(color,selected));
    let band=egui::Rect::from_min_max(block.left_top(),egui::pos2(block.right(),block.top()+3.0));
    p.rect_filled(band,egui::CornerRadius{nw:3,ne:3,sw:0,se:0},color);
    p.rect_stroke(block,3.0,egui::Stroke::new(if selected {1.5} else {1.0},
        if selected {egui::Color32::WHITE} else {color.gamma_multiply(0.45)}),egui::StrokeKind::Inside);
    if block.width()>28.0 {
        let clip=p.with_clip_rect(block.shrink2(egui::vec2(4.0,1.0)));
        let galley=clip.layout_no_wrap(label.to_owned(),egui::FontId::proportional(11.5),theme::TEXT);
        let width=galley.size().x;
        let y=block.center().y+1.5;
        clip.galley(egui::pos2(block.left()+7.0,y-galley.size().y*0.5),galley,theme::TEXT);
        if !detail.is_empty() && block.width()>width+60.0 {
            clip.text(egui::pos2(block.left()+15.0+width,y),egui::Align2::LEFT_CENTER,detail,
                egui::FontId::proportional(10.5),theme::TEXT_FAINT);
        }
    }
}

impl App {
    /// Character motion key on clip `i` at timeline `time`.
    fn studio_add_motion_key_at(&mut self, i:usize, time:f64) {
        use crate::app::render_studio_timeline::StudioInterpolation;
        let smooth=self.render_studio.timeline.interpolation!=StudioInterpolation::Linear;
        let Some(clip)=self.render_studio.timeline.clips.get_mut(i) else {return};
        let mut key=clip.motion_at(time,smooth);
        key.time=clip.source_key_time(time);
        let k=clip.add_motion_key(key);
        self.render_studio.timeline.select_item(StudioItem::Motion(i,k),false,false);
        self.profile_preview_key=None;
    }
    /// Speed key on animation clip `i` at `time`, keeping the current rate
    /// (so timing doesn't move until the key is edited).
    fn studio_add_clip_speed_key_at(&mut self, i:usize, time:f64) {
        use crate::app::render_studio_timeline::{studio_speed_at,studio_add_speed_key,StudioSpeedSelection};
        let Some(clip)=self.render_studio.timeline.clips.get_mut(i) else {return};
        let source=clip.source_key_time(time);
        let rate=studio_speed_at(&clip.speed_keys,source);
        studio_add_speed_key(&mut clip.speed_keys,source,rate);
        let lane=clip.lane;
        if let Some(k)=clip.speed_keys.iter().position(|k|(k.time-source).abs()<0.00001) {
            self.render_studio.timeline.select_speed_key(StudioSpeedSelection::Animation(i,k));
        }
        if !self.render_studio.timeline.speed_expanded(lane) {self.render_studio.timeline.toggle_speed_track(lane);}
        self.profile_preview_key=None;
    }
    fn studio_add_demo_speed_key_at(&mut self, i:usize, time:f64) {
        use crate::app::render_studio_timeline::{studio_speed_at,studio_add_speed_key,StudioSpeedSelection};
        let Some(clip)=self.render_studio.timeline.demos.get_mut(i) else {return};
        let source=clip.source_time_ms(time).max(0) as f64/1000.0;
        let rate=studio_speed_at(&clip.speed_keys,source);
        studio_add_speed_key(&mut clip.speed_keys,source,rate);
        if let Some(k)=clip.speed_keys.iter().position(|k|(k.time-source).abs()<0.00001) {
            self.render_studio.timeline.select_speed_key(StudioSpeedSelection::Demo(i,k));
        }
        let layer=self.render_studio.timeline.demos[i].layer;
        if !self.render_studio.timeline.demo_speed_expanded(layer) {self.render_studio.timeline.toggle_demo_speed(layer);}
        self.profile_preview_key=None;
    }
    fn studio_add_demo_volume_key_at(&mut self, i:usize, time:f64) {
        let Some(clip)=self.render_studio.timeline.demos.get_mut(i) else {return};
        if time<clip.start || time>=clip.end() {return;}
        let source=f64::from(clip.source_time_ms(time))/1000.0;
        clip.audio.add_key(source,1.0);
        self.render_studio.timeline.clear_selection();
        self.render_studio.timeline.selected_demo=Some(i);
        self.render_studio.audio.selected_demo_audio=Some(i);
    }
    pub(super) fn studio_add_audio_volume_key_at(&mut self, track:usize, i:usize, time:f64) {
        let count=self.render_studio.timeline.audio_tracks.len();
        let Some(clip)=self.render_studio.timeline.audio_tracks.get_mut(track).and_then(|t|t.clips.get_mut(i)) else {return};
        let local=(time-clip.start).clamp(0.0,clip.duration());
        let gain=clip.automation_at(local);
        let k=clip.add_key(local,gain);
        self.render_studio.timeline.audio_expanded.resize(count,false);
        self.render_studio.timeline.audio_expanded[track]=true;
        self.render_studio.timeline.clear_selection();
        self.render_studio.audio.selected=Some((track,i));
        self.render_studio.audio.selected_track=Some(track);
        self.render_studio.audio.selected_key=Some(k);
    }
    fn studio_demo_index_at(&self, time:f64)->Option<usize> {
        let clip=self.render_studio.timeline.demo_at(time)?;
        self.render_studio.timeline.demos.iter().position(|c|std::ptr::eq(c,clip))
    }

    /// Right-click on empty track space: keys for the track under the
    /// pointer, at the pointer's time.
    fn egui_studio_track_key_items(&mut self, ui:&mut egui::Ui, row:Option<usize>, time:f64) {
        use crate::app::render_studio_timeline::StudioTrackRow;
        let kind=row.and_then(|r|self.render_studio.timeline.row_kind(r));
        let item=|ui:&mut egui::Ui,enabled:bool,label:&str,why:&str| {
            ui.add_enabled(enabled,egui::Button::new(label)).on_disabled_hover_text(why).clicked()
        };
        match kind {
            Some(StudioTrackRow::Light(layer,light)) => {
                if ui.button("Add light key here").clicked(){self.studio_add_light_key(layer,light,time);ui.close();}
            }
            Some(StudioTrackRow::Character(lane)|StudioTrackRow::Motion(lane)) => {
                let clip=self.render_studio.timeline.clips.iter().position(|c|c.lane==lane && time>=c.start && time<=c.end());
                if item(ui,clip.is_some(),"Add motion key here","No animation clip on this track at this time") {
                    if let Some(i)=clip {self.studio_add_motion_key_at(i,time);}
                    ui.close();
                }
            }
            Some(StudioTrackRow::Speed(lane)) => {
                let clip=self.render_studio.timeline.clips.iter().position(|c|c.lane==lane && time>=c.start && time<=c.end());
                if item(ui,clip.is_some(),"Add speed key here","No animation clip on this track at this time") {
                    if let Some(i)=clip {self.studio_add_clip_speed_key_at(i,time);}
                    ui.close();
                }
            }
            Some(StudioTrackRow::Demo(layer)|StudioTrackRow::DemoSpeed(layer)) => {
                let demo=self.render_studio.timeline.demos.iter().position(|c|c.layer==layer && time>=c.start && time<c.end());
                if item(ui,demo.is_some(),"Add speed key here","No demo footage at this time") {
                    if let Some(i)=demo {self.studio_add_demo_speed_key_at(i,time);}
                    ui.close();
                }
            }
            Some(StudioTrackRow::DemoAudio(layer)) => {
                let demo=self.render_studio.timeline.demos.iter().position(|c|c.layer==layer && time>=c.start && time<c.end());
                if item(ui,demo.is_some(),"Add volume key here","No demo footage at this time") {
                    if let Some(i)=demo {self.studio_add_demo_volume_key_at(i,time);}
                    ui.close();
                }
            }
            Some(StudioTrackRow::Audio(track)|StudioTrackRow::AudioVolume(track)) => {
                let clip=self.render_studio.timeline.audio_tracks.get(track)
                    .and_then(|t|t.clips.iter().position(|c|time>=c.start && time<c.end()));
                if item(ui,clip.is_some(),"Add volume key here","No audio clip on this track at this time") {
                    if let Some(i)=clip {self.studio_add_audio_volume_key_at(track,i,time);}
                    ui.close();
                }
            }
            _ => {}
        }
        if ui.button("Add camera key here").clicked() {
            self.capture_studio_preview_camera(time);ui.close();
        }
        if self.render_studio.timeline.demo_at(time).is_some() && ui.button("Add Director shot here").clicked() {
            let client=self.render_studio.director_new_client;
            let preset=self.render_studio.director_new_preset;
            if let Some(i)=self.studio_add_director_shot(preset,client,time) {
                let id=self.render_studio.timeline.director_shots[i].id;
                self.studio_select_director(id);
            }
            ui.close();
        }
    }
}

impl App {
    /// Esc inside Render Studio, innermost first: leave prompt, open popup or
    /// text field, dialogs, source preview, viewer mouse-look, selection.
    /// Only with nothing left to cancel does it ask before leaving.
    pub(in crate::app) fn render_studio_escape(&mut self) {
        self.egui_repaint_requested = true;
        let ctx = self.egui_ctx.clone();
        if self.render_studio.show_new_project {
            self.render_studio.show_new_project=false;
            return;
        }
        if self.render_studio.confirm_leave {
            self.render_studio.confirm_leave = false;
            return;
        }
        if self.render_studio.director_popup_open {
            self.render_studio.director_popup_open=false;
            egui::Popup::close_all(&ctx);
            return;
        }
        if self.render_studio.background_popup_open {
            self.render_studio.background_popup_open=false;
            egui::Popup::close_all(&ctx);
            return;
        }
        if egui::Popup::is_any_open(&ctx) || ctx.text_edit_focused() {
            // egui closes the popup / drops focus on the same Esc.
            ctx.memory_mut(|m| if let Some(id) = m.focused() { m.surrender_focus(id); });
            return;
        }
        let studio = &mut self.render_studio;
        if studio.spatial.camera_handle_drag.take().is_some(){return;}
        if studio.light_ui.position_drag.take().is_some(){return;}
        if studio.show_render && studio.export.is_none() && studio.still.is_none() { studio.show_render = false; return; }
        if studio.model_picker_lane.is_some() { studio.model_picker_lane = None; return; }
        if studio.show_open_project { studio.show_open_project = false; return; }
        if studio.show_save_as { studio.show_save_as = false; return; }
        if studio.graphics_settings_open { studio.graphics_settings_open = false; return; }
        if studio.cinematic_preview.is_some() {
            studio.cinematic_preview = None;
            self.profile_preview_key = None;
            return;
        }
        if self.render_studio.navigation.selected {
            self.render_studio.navigation.selected = false;
            self.set_studio_mouse_look(false);
            return;
        }
        let studio = &mut self.render_studio;
        if studio.light_ui.selection.is_some() {studio.light_ui=Default::default();return;}
        let selected = studio.director_selected.is_some() || !studio.timeline.selected_items().is_empty()
            || studio.timeline.selected_demo.is_some() || studio.selected_background
            || studio.audio.selected.is_some() || studio.audio.selected_track.is_some()
            || studio.timeline.selected_speed.is_some();
        if selected {
            studio.director_selected = None;
            studio.timeline.clear_selection();
            studio.timeline.selected_demo = None;
            studio.timeline.selected_speed = None;
            studio.selected_background = false;
            studio.audio.selected = None;
            studio.audio.selected_track = None;
            studio.audio.selected_demo_audio = None;
            return;
        }
        studio.confirm_leave = true;
    }

    /// "Leave Render Studio?" prompt.
    fn egui_render_studio_leave_dialog(&mut self, ctx:&egui::Context) {
        if !self.render_studio.confirm_leave {return;}
        let exporting=self.studio_locked();
        let mut choice=None;
        let (close,_)=Self::studio_modal(ctx,"studio_confirm_leave","Leave Render Studio?",400.0,|ui| {
            sui::hint(ui,if exporting {"An export is running; leaving cancels it."}
                else {"The timeline stays in memory until you quit the game. Save the project to keep it on disk."});
            ui.add_space(8.0);
            choice=sui::button_row(ui,&[(None,"Stay",ButtonKind::Normal,true),
                (Some(Icon::Save),"Save and leave",ButtonKind::Normal,!exporting),
                (None,"Leave",ButtonKind::Primary,true)]);
        });
        match choice {
            Some(1)=>{self.save_render_studio_project();self.render_studio.confirm_leave=false;self.frontend_back();}
            Some(2)=>{self.render_studio.confirm_leave=false;self.frontend_back();}
            Some(_)=>self.render_studio.confirm_leave=false,
            None if close=>self.render_studio.confirm_leave=false,
            None=>{}
        }
    }
}

impl App {
    /// "Add track" menu items (header + Track button, timeline right-click).
    pub(super) fn egui_studio_add_track_items(&mut self, ui:&mut egui::Ui) {
        if ui.add_enabled(self.render_studio.timeline.light_layers.len()<crate::app::render_studio_lights::MAX_LAYERS,egui::Button::new("Light layer")).clicked(){self.studio_add_light_layer();ui.close();}
        let timeline=&mut self.render_studio.timeline;
        if ui.add_enabled(timeline.lanes<12,egui::Button::new("Character track")).clicked() {
            let lane=(0..timeline.lanes).find(|&l|!timeline.lane_shown(l)).unwrap_or(timeline.lanes);
            if lane==timeline.lanes {
                timeline.lanes+=1;
                timeline.lane_models.resize(timeline.lanes,String::new());
                timeline.lane_sabers.resize(timeline.lanes,None);
                timeline.lane_char_colors.resize(timeline.lanes,None);
                timeline.lane_visible.resize(timeline.lanes,true);
                timeline.lane_speed_expanded.resize(timeline.lanes,false);
            }
            timeline.lane_pinned.resize(timeline.lanes,false);
            timeline.lane_pinned[lane]=true;
            self.render_studio.status="Added a character track: choose its model, then drop animations on it.".into();
            ui.close();
        }
        if ui.button("Demo layer").clicked() {
            let layer=timeline.demo_layer_for_drop();
            timeline.demo_layer_pinned.resize(timeline.demo_layer_pinned.len().max(layer+1),false);
            timeline.demo_layer_pinned[layer]=true;
            self.render_studio.status="Added a demo layer: the top visible layer plays where layers overlap.".into();
            ui.close();
        }
        if ui.add_enabled(timeline.audio_tracks.len()<32,egui::Button::new("Audio track")).clicked() {
            let n=timeline.audio_tracks.len();
            timeline.audio_tracks.push(crate::app::render_studio_audio::StudioAudioTrack::new(n+1));
            timeline.audio_expanded.push(false);
            ui.close();
        }
    }
}

/// Stable identity of a library row across frames.
fn studio_library_key(asset:&StudioMediaDrag)->String {
    if let Some(game)=&asset.demo_game {format!("demo:{game}/{}",asset.name)}
    else if let Some(id)=&asset.emote_id {format!("emote:{id}")}
    else if let Some(source)=&asset.cinematic {format!("cinematic:{}/{}",source.gla,source.animation)}
    else {format!("anim:{}",asset.name)}
}
