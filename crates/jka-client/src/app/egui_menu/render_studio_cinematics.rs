use super::App;
use super::render_studio_ui::palette as theme;
use super::render_studio_ui::{self as sui, Icon};
use crate::app::render_studio::StudioMediaDrag;
use std::collections::BTreeMap;

impl App {
    /// Single-player cinematic actor animations, grouped by source set.
    /// Clicking a row opens it in the viewer's source mode; the transport
    /// bar then offers Add at playhead / Back to timeline.
    pub(super) fn egui_studio_cinematic_library(&mut self,ui:&mut egui::Ui,locked:bool) {
        self.ensure_studio_cinematics();
        ui.spacing_mut().item_spacing.y=5.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x=4.0;
            ui.allocate_ui(egui::vec2((ui.available_width()-30.0).max(60.0),26.0),|ui| {
                sui::search_field(ui,&mut self.render_studio.cinematics.search,"Search map, set or CIN number");
            });
            if sui::icon_button(ui,Icon::Undo,true,false,"Reload cinematic animation sets").clicked() {
                self.render_studio.cinematics.key=None;
            }
        });
        let Some(result)=self.render_studio.cinematics.catalog.clone() else {
            ui.horizontal(|ui|{ui.spinner();sui::hint(ui,"Reading cinematic animation sets…");});
            return;
        };
        let catalog=match result {Ok(catalog)=>catalog,Err(error)=>{sui::callout(ui,&error,theme::DANGER);return;}};
        let needle=self.render_studio.cinematics.search.trim().to_ascii_lowercase();
        let mut groups:BTreeMap<String,Vec<_>>=BTreeMap::new();
        for clip in &catalog.clips {
            if needle.is_empty() || format!("{} {} {}",clip.label,clip.source.animation,clip.source.gla).to_ascii_lowercase().contains(&needle) {
                groups.entry(clip.set_label.clone()).or_default().push(clip);
            }
        }
        sui::hint(ui,&if groups.is_empty() {"No matching cinematic animations.".to_owned()} else {
            format!("{} clips in {} sets · actor animation only (no dialogue, props or scripted cameras)",
                groups.values().map(Vec::len).sum::<usize>(),groups.len())});
        for (group,clips) in groups {
            let id=ui.make_persistent_id(("studio_cinematic_set",&group));
            let mut state=egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(),id,false);
            if !needle.is_empty() {state.set_open(true);}
            let (rect,r)=ui.allocate_exact_size(egui::vec2(ui.available_width(),24.0),egui::Sense::click());
            if r.clicked() {state.toggle(ui);}
            let open=state.is_open();
            state.store(ui.ctx());
            let p=ui.painter();
            p.rect_filled(rect,3.0,if r.hovered() {theme::CONTROL} else {egui::Color32::TRANSPARENT});
            sui::paint_icon(p,egui::pos2(rect.left()+10.0,rect.center().y),
                if open {Icon::ChevronDown} else {Icon::ChevronRight},theme::TEXT_FAINT);
            p.with_clip_rect(rect).text(egui::pos2(rect.left()+22.0,rect.center().y),egui::Align2::LEFT_CENTER,
                &group,egui::FontId::proportional(12.0),theme::TEXT_DIM);
            p.text(egui::pos2(rect.right()-8.0,rect.center().y),egui::Align2::RIGHT_CENTER,
                clips.len().to_string(),egui::FontId::proportional(11.0),theme::TEXT_FAINT);
            if open {
                ui.spacing_mut().item_spacing.y=1.0;
                for clip in clips {
                    let _=self.egui_render_studio_media_tile(ui,StudioMediaDrag {name:clip.label.clone(),emote_id:None,cinematic:Some(clip.source.clone()),demo_game:None},locked);
                }
                ui.spacing_mut().item_spacing.y=5.0;
            }
        }
        if !catalog.diagnostics.is_empty() {
            sui::section(ui,"studio_cinematic_diagnostics","Unavailable animation sets",None,false,|ui| {
                for diagnostic in &catalog.diagnostics {sui::hint(ui,diagnostic);}
            });
        }
    }
}
