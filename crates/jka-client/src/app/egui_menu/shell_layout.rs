//! Taffy layout of the menu chrome shared by every egui page: header, the
//! Setup tab strip, the Video rail, the page body and the footer strip. The
//! regions are computed once per frame here and handed to the painters in
//! `shell.rs` / `frontend_pages.rs`, which used to stack egui Panels.
use crate::ui::flex::{Container, Flex};
use egui::{pos2, Rect};

pub(in crate::app::egui_menu) const HEADER_H: f32 = 64.0;
pub(in crate::app::egui_menu) const TABS_H: f32 = 40.0;
pub(in crate::app::egui_menu) const RAIL_W: f32 = 196.0;

#[derive(Clone, Copy, Debug)]
pub(in crate::app::egui_menu) struct Shell {
    pub header: Rect,
    pub tabs: Option<Rect>,
    pub rail: Option<Rect>,
    pub body: Rect,
    pub footer: Rect,
}

#[derive(Clone, Copy, Debug)]
pub(in crate::app::egui_menu) struct ShellSpec {
    pub tabs: bool,
    pub rail: bool,
    pub footer_h: f32,
}

fn rect(bounds: [f32; 4], origin: egui::Pos2) -> Rect {
    Rect::from_min_size(pos2(origin.x + bounds[0], origin.y + bounds[1]), egui::vec2(bounds[2], bounds[3]))
}

pub(in crate::app::egui_menu) fn compute(screen: Rect, spec: ShellSpec) -> Shell {
    let (w, h) = (screen.width().max(1.0), screen.height().max(1.0));
    let mut flex = Flex::new();
    let header = flex.band(HEADER_H);
    let tabs = spec.tabs.then(|| flex.band(TABS_H));
    let rail = spec.rail.then(|| flex.node(Container::column().width(RAIL_W), &[]));
    let body = flex.spring();
    // Rail and body share the middle row, which takes all remaining height.
    let middle_children: Vec<_> = rail.into_iter().chain([body]).collect();
    let mut middle_box = Container::row();
    middle_box.align = taffy::prelude::AlignItems::STRETCH;
    let middle = flex.node(middle_box, &middle_children);
    let footer = flex.band(spec.footer_h);
    let mut children = vec![header];
    children.extend(tabs);
    children.extend([middle, footer]);
    let root = flex.node(Container::column().width(w).height(h), &children);
    // The middle row is the only flexible child of the column.
    flex.set_grow(middle, 1.0);
    flex.compute(root, w, h);
    let origin = screen.min;
    Shell {
        header: rect(flex.bounds(header), origin),
        tabs: tabs.map(|node| rect(flex.bounds(node), origin)),
        rail: rail.map(|node| rect(flex.bounds(node), origin)),
        body: rect(flex.bounds(body), origin),
        footer: rect(flex.bounds(footer), origin),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(1600.0, 900.0))
    }

    #[test]
    fn regions_tile_the_screen_top_to_bottom() {
        let shell = compute(screen(), ShellSpec { tabs: true, rail: true, footer_h: 72.0 });
        let tabs = shell.tabs.unwrap();
        let rail = shell.rail.unwrap();
        assert_eq!(shell.header.top(), 0.0);
        assert_eq!(tabs.top(), shell.header.bottom());
        assert_eq!(shell.body.top(), tabs.bottom());
        assert_eq!(rail.right(), shell.body.left());
        assert_eq!(shell.body.bottom(), shell.footer.top());
        assert_eq!(shell.footer.bottom(), 900.0);
        assert_eq!(shell.body.right(), 1600.0);
    }

    #[test]
    fn without_tabs_or_rail_the_body_takes_their_space() {
        let shell = compute(screen(), ShellSpec { tabs: false, rail: false, footer_h: 40.0 });
        assert!(shell.tabs.is_none() && shell.rail.is_none());
        assert_eq!(shell.body.top(), HEADER_H);
        assert_eq!(shell.body.left(), 0.0);
        assert_eq!(shell.body.height(), 900.0 - HEADER_H - 40.0);
    }
}

impl crate::app::App {
    /// This frame's chrome regions for the page being shown.
    pub(in crate::app::egui_menu) fn menu_shell(&self, ctx: &egui::Context) -> Shell {
        use crate::app::FrontendPage;
        use crate::ui::OverlayMode;
        let video = self.egui_display_overlay() == OverlayMode::Video;
        let studio = self.front_end && !video && self.frontend_page == FrontendPage::RenderStudio;
        let footer_h = if studio {
            0.0
        } else if self.front_end {
            crate::neon_menu::footer::HEIGHT
        } else {
            super::theme::FOOTER_H
        };
        compute(ctx.content_rect(), ShellSpec {
            tabs: video,
            rail: video && self.setup_selected == super::SETUP_TAB_VIDEO,
            footer_h,
        })
    }
}

/// Run `add` in a child `Ui` confined to `rect` (one shell region).
pub(in crate::app::egui_menu) fn region<R>(
    root: &mut egui::Ui,
    id: &str,
    rect: Rect,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    root.scope_builder(
        egui::UiBuilder::new()
            .id_salt(id)
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
        |ui| {
            ui.set_clip_rect(rect.intersect(ui.clip_rect()));
            add(ui)
        },
    )
    .inner
}
