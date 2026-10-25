use super::{App,theme};
use crate::app::render_studio::StudioMediaDrag;
use std::collections::BTreeMap;

impl App {
    pub(super) fn egui_studio_cinematic_library(&mut self,ui:&mut egui::Ui,locked:bool) {
        self.ensure_studio_cinematics();
        ui.label(egui::RichText::new("SINGLE-PLAYER CINEMATICS").strong().color(theme::ACCENT));
        ui.small("Click to preview; drag onto a character lane to place. These are actor animations. Dialogue, props, scene cameras and scripted movement are not included.");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.render_studio.cinematics.search)
                .hint_text("Search map, source set or CIN number").desired_width((ui.available_width()-65.).max(80.)));
            if ui.button("Reload").clicked() {self.render_studio.cinematics.key=None;}
        });
        let Some(result)=self.render_studio.cinematics.catalog.clone() else {ui.spinner();ui.small("Reading cinematic animation sets...");return;};
        let catalog=match result {Ok(catalog)=>catalog,Err(error)=>{ui.colored_label(egui::Color32::LIGHT_RED,error);return;}};
        let needle=self.render_studio.cinematics.search.trim().to_ascii_lowercase();
        let mut groups:BTreeMap<String,Vec<_>>=BTreeMap::new();
        for clip in &catalog.clips {
            if needle.is_empty() || format!("{} {} {}",clip.label,clip.source.animation,clip.source.gla).to_ascii_lowercase().contains(&needle) {
                groups.entry(clip.set_label.clone()).or_default().push(clip);
            }
        }
        ui.small(format!("{} clips in {} sets",groups.values().map(Vec::len).sum::<usize>(),groups.len()));
        if groups.is_empty() {ui.small("No matching cinematic animations.");}
        for (group,clips) in groups {
            egui::CollapsingHeader::new(format!("{group} ({})",clips.len())).id_salt(("studio_cinematic_set",&group))
                .open(if needle.is_empty(){None}else{Some(true)}).show(ui,|ui| {
                    for clip in clips {
                        self.egui_render_studio_media_tile(ui,StudioMediaDrag {name:clip.label.clone(),emote_id:None,cinematic:Some(clip.source.clone())},locked);
                    }
                });
        }
        if !catalog.diagnostics.is_empty() {ui.collapsing("Unavailable animation sets",|ui|for diagnostic in &catalog.diagnostics {ui.small(diagnostic);});}
    }

    pub(super) fn egui_studio_cinematic_preview_toolbar(&mut self,ui:&mut egui::Ui,viewport:egui::Rect)->bool {
        let rect=egui::Rect::from_min_size(viewport.min+egui::vec2(8.,7.),egui::vec2((viewport.width()-16.).max(1.),78.));
        let busy=ui.ctx().input(|i|i.pointer.interact_pos().is_some_and(|p|rect.contains(p)));
        let mut back=false;let mut add=None;let mut changed=false;
        ui.scope_builder(egui::UiBuilder::new().id_salt("studio_source_preview").max_rect(rect),|ui| {
            ui.set_clip_rect(rect);
            egui::Frame::new().fill(egui::Color32::from_black_alpha(220)).inner_margin(6).corner_radius(4.).show(ui,|ui| {
                let Some(preview)=self.render_studio.cinematic_preview.as_mut() else{return;};
                if preview.playing && !preview.emote.looping && preview.time+preview.started.elapsed().as_secs_f64()>=preview.clip.duration() {
                    preview.seek(preview.clip.duration());changed=true;
                }
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(egui::RichText::new(format!("PREVIEW: {}",preview.clip.label)).color(theme::ACCENT)).truncate());
                    if ui.button("Back to timeline").clicked() {back=true;}
                    if ui.button("Add at playhead").clicked() {add=Some((preview.clip.clone(),preview.lane));}
                });
                ui.horizontal(|ui| {
                    if ui.button(if preview.playing {"Pause"}else{"Play"}).clicked() {preview.toggle();changed=true;}
                    let duration=preview.clip.duration();let mut time=preview.elapsed();
                    if ui.add(egui::Slider::new(&mut time,0.0..=duration).show_value(false)
                        .custom_formatter(|t,_|format!("{t:.2}s"))).changed() {preview.seek(time);changed=true;}
                    ui.small(format!("{time:.2} / {duration:.2}s"));
                    changed|=ui.checkbox(&mut preview.emote.looping,"Loop preview").changed();
                });
                ui.small(format!("Frames {}..{} · {:.2} FPS · Space: play/pause · arrows: step",preview.clip.first_frame,preview.clip.end_frame()-1,preview.clip.fps));
            });
        });
        if back {self.render_studio.cinematic_preview=None;changed=true;}
        if let Some((clip,lane))=add {
            let time=self.render_studio.timeline_time();
            self.render_studio_drop_media(StudioMediaDrag {name:clip.label,emote_id:None,cinematic:Some(clip.source)},time,lane);changed=true;
        }
        if changed {self.profile_preview_key=None;ui.ctx().request_repaint();}
        busy
    }
}
