//! Taffy submenu input: just hit testing, keyboard navigation and dispatch.
//! The *main-menu* WGPU HDR neon/CRT renderer owns every visible pixel.
use super::{App, FrontendPage, RenderCommand};
use crate::neon_menu::{submenu, MenuFrame, MenuPage};
use std::time::{Duration, Instant};

const ACTIVATE_DELAY: Duration = Duration::from_millis(170);

const PLAY: [&str; 3] = ["JOIN A GAME", "SOLO GAME", "PLAY DEMO"];
const DEV: [&str; 4] = ["ASSET VIEWER", "RENDER STUDIO", "MAP VIEWER", "PK3 MANAGER"];

impl App {
    pub(in crate::app::egui_menu) fn draw_neon_frontend_submenu(
        &mut self, ui: &mut egui::Ui, page_kind: MenuPage,
    ) {
        let play = page_kind == MenuPage::Play;
        if page_kind == MenuPage::Main { return; }
        let count = if play { PLAY.len() } else { DEV.len() };
        let now = Instant::now();
        if self.neon_menu.submenu_page != Some(page_kind) {
            self.neon_menu.submenu_page = Some(page_kind);
            self.neon_menu.submenu_pending = None;
            self.neon_menu.selected = 0;
            self.neon_menu.active = None;
            self.neon_menu.pressed = 0;
            self.neon_menu.press_serial = 0;
            self.neon_menu.shown_at = None;
        }
        let area = ui.max_rect();
        let page = if play {
            submenu::compute(area.width(), area.height(), &PLAY)
                .map(|page| (page.lines.to_vec(), page.hit.to_vec()))
        } else {
            submenu::compute(area.width(), area.height(), &DEV)
                .map(|page| (page.lines.to_vec(), page.hit.to_vec()))
        };
        let Some((lines, hits)) = page else { return };
        let rect = |r: [f32; 4]| egui::Rect::from_min_max(
            area.min + egui::vec2(r[0] * area.width(), r[1] * area.height()),
            area.min + egui::vec2(r[2] * area.width(), r[3] * area.height()),
        );
        let mut selected = usize::from(self.neon_menu.selected).min(count - 1);
        let mut active = self.neon_menu.active.map(usize::from);
        let mut hovered = None;
        let mut activate = None;
        let (moved, pointer_gone) = ui.input(|i| (i.pointer.is_moving(), i.pointer.hover_pos().is_none()));
        for (index, hit) in hits.iter().enumerate() {
            let response = ui.interact(rect(*hit), ui.make_persistent_id(("neon_submenu", page_kind, index)), egui::Sense::click());
            if response.hovered() {
                hovered = Some(index);
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.clicked() { activate = Some(index); }
        }
        if moved || activate.is_some() { active = hovered; }
        else if pointer_gone && hovered.is_none() { active = None; }
        if !self.frontend_quit_confirmation {
            ui.input(|input| {
                if input.key_pressed(egui::Key::ArrowDown) {
                    selected = active.map_or(selected, |a| (a + 1) % count);
                    active = Some(selected);
                }
                if input.key_pressed(egui::Key::ArrowUp) {
                    selected = active.map_or(selected, |a| (a + count - 1) % count);
                    active = Some(selected);
                }
                if input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space) {
                    activate = Some(active.unwrap_or(selected));
                }
            });
        } else { activate = None; active = None; }
        if let Some(index) = active { selected = index; }
        self.neon_menu.selected = selected as u8;
        self.neon_menu.active = active.map(|i| i as u8);
        if let Some(index) = activate.filter(|_| !self.frontend_quit_confirmation && self.neon_menu.submenu_pending.is_none()) {
            self.neon_menu.pressed = index as u8;
            self.neon_menu.press_serial = self.neon_menu.press_serial.wrapping_add(1);
            self.neon_menu.submenu_pending = Some((page_kind, index, now));
        }
        self.neon_menu.shown_at.get_or_insert(now);
        let frame = MenuFrame {
            page: page_kind,
            layout: {
                let mut layout = crate::neon_menu::layout::MenuLayout::default();
                for (index, line) in lines.iter().enumerate() { layout.items[index] = *line; }
                for (index, hit) in hits.iter().enumerate() { layout.hit[index] = *hit; }
                layout
            },
            selected: selected as u8,
            active: active.map(|a| a as u8),
            pressed: self.neon_menu.pressed,
            press_serial: self.neon_menu.press_serial,
            revealed: true,
            live_scene_blend: self.frontend_scene_handoff(now),
        };
        if self.neon_menu.sent.as_ref() != Some(&frame) {
            self.render_command(RenderCommand::SetNeonMenu(Some(frame.clone())));
            self.neon_menu.sent = Some(frame.clone());
        }
        if frame.live_scene_blend > 0.0 && frame.live_scene_blend < 1.0 {
            ui.ctx().request_repaint();
        }
        if let Some((pending_page, index, since)) = self.neon_menu.submenu_pending {
            if pending_page == page_kind && now.duration_since(since) >= ACTIVATE_DELAY {
                self.neon_menu.submenu_pending = None;
                match page_kind {
                    MenuPage::Play => self.activate_play_tool(index),
                    MenuPage::DeveloperTools => self.activate_developer_tool(index),
                    MenuPage::Main => {}
                }
            } else {
                ui.ctx().request_repaint_after(ACTIVATE_DELAY.saturating_sub(now.duration_since(since)));
            }
        }
    }

    fn activate_play_tool(&mut self, index: usize) {
        match index {
            0 => {
                self.server_browser.sort = crate::server_browser::BrowserSort::LastVisited;
                self.server_browser.sort_ascending = false;
                self.frontend_page = FrontendPage::ServerBrowser;
                self.egui_repaint_requested = true;
            }
            1 => {
                self.frontend_page = FrontendPage::SoloGame;
                self.egui_repaint_requested = true;
            }
            2 => {
                self.frontend_page = FrontendPage::PlayDemo;
                self.egui_repaint_requested = true;
            }
            _ => {}
        }
    }

    fn activate_developer_tool(&mut self, index: usize) {
        match index {
            0 => {
                self.frontend_page = FrontendPage::AssetViewer;
                self.render_command(RenderCommand::SetAssetPreviewBackground(None));
                self.profile_background_requested = None;
                self.render_command(RenderCommand::SetAssetPreviewMode(true));
                self.render_command(RenderCommand::SetAssetPreviewViewport(None));
                self.asset_preview_viewport_key = None;
                self.clear_asset_preview_runtime();
                self.egui_repaint_requested = true;
            }
            1 => self.open_render_studio(),
            2 => {
                self.frontend_page = FrontendPage::MapViewer;
                self.egui_repaint_requested = true;
            }
            3 => {
                self.frontend_page = FrontendPage::Pk3Manager;
                self.pk3_manager_loaded = false;
                self.egui_repaint_requested = true;
            }
            _ => {}
        }
    }
}
