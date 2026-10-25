//! Frontend Main page input. The renderer draws the page (renderer/neon_menu.rs);
//! egui only supplies pointer and keyboard input here. Hit targets come from
//! the same Taffy layout the renderer draws with.
use super::{App, FrontendPage, OverlayMode, RenderCommand, SETUP_TAB_VIDEO};
use crate::neon_menu::{layout, Item, MenuFrame, ITEMS, ITEM_COUNT};
use std::time::{Duration, Instant};

/// Pause between the click flash and leaving the page.
const ACTIVATE_DELAY: Duration = Duration::from_millis(170);

#[derive(Default)]
pub(in crate::app) struct NeonMenuState {
    selected: u8,
    active: Option<u8>,
    pressed: u8,
    press_serial: u32,
    pending: Option<(Item, Instant)>,
    pub(in crate::app) renderer_ready: bool,
    sent: Option<MenuFrame>,
}

impl App {
    pub(in crate::app::egui_menu) fn draw_neon_main_menu(&mut self, ui: &mut egui::Ui) {
        let now = Instant::now();
        let screen = ui.max_rect();
        let Some(page) = layout::compute(screen.width(), screen.height()) else { return };
        let to_rect = |r: [f32; 4]| {
            egui::Rect::from_min_max(
                screen.min + egui::vec2(r[0] * screen.width(), r[1] * screen.height()),
                screen.min + egui::vec2(r[2] * screen.width(), r[3] * screen.height()),
            )
        };

        let mut selected = usize::from(self.neon_menu.selected).min(ITEM_COUNT - 1);
        let mut active = self.neon_menu.active.map(usize::from);
        let mut activate = None;
        let (pointer_moved, pointer_gone) =
            ui.input(|input| (input.pointer.is_moving(), input.pointer.hover_pos().is_none()));
        let mut hovered = None;
        for (index, hit) in page.hit.iter().enumerate() {
            let response = ui.interact(to_rect(*hit), ui.make_persistent_id(("neon_main_entry", index)), egui::Sense::click());
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                hovered = Some(index);
            }
            if response.clicked() {
                hovered = Some(index);
                activate = Some(index);
            }
        }
        // The pointer owns the focus while it moves; keyboard focus survives
        // until it does. Leaving every entry lets the glass fog over again.
        if pointer_moved || activate.is_some() {
            active = hovered;
        } else if pointer_gone && hovered.is_none() {
            active = None;
        }
        ui.input(|input| {
            if input.key_pressed(egui::Key::ArrowDown) {
                selected = active.map_or(selected, |a| (a + 1) % ITEM_COUNT);
                active = Some(selected);
            }
            if input.key_pressed(egui::Key::ArrowUp) {
                selected = active.map_or(selected, |a| (a + ITEM_COUNT - 1) % ITEM_COUNT);
                active = Some(selected);
            }
            if input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space) {
                activate = Some(active.unwrap_or(selected));
            }
        });
        if let Some(index) = active {
            selected = index;
        }
        self.neon_menu.selected = selected as u8;
        self.neon_menu.active = active.map(|index| index as u8);
        if let Some(index) = activate.filter(|_| self.neon_menu.pending.is_none()) {
            self.neon_menu.pressed = index as u8;
            self.neon_menu.press_serial = self.neon_menu.press_serial.wrapping_add(1);
            self.neon_menu.pending = Some((ITEMS[index].item, now));
        }

        // The page is interactive and its power-on animation starts immediately.
        // Only the background crossfade follows the live scene's readiness.
        let frame = MenuFrame {
            layout: page,
            selected: self.neon_menu.selected,
            active: self.neon_menu.active,
            pressed: self.neon_menu.pressed,
            press_serial: self.neon_menu.press_serial,
            revealed: true,
            live_scene_blend: self.frontend_scene_handoff(now),
        };
        if self.neon_menu.sent.as_ref() != Some(&frame) {
            self.render_command(RenderCommand::SetNeonMenu(Some(frame.clone())));
            self.neon_menu.sent = Some(frame.clone());
        }
        // The menu is no longer egui-painted after WGPU takes ownership of
        // its background. Keep advancing the scene handoff even then.
        if frame.live_scene_blend > 0.0 && frame.live_scene_blend < 1.0 {
            ui.ctx().request_repaint();
        }
        if let Some((item, at)) = self.neon_menu.pending {
            if now.duration_since(at) >= ACTIVATE_DELAY {
                self.neon_menu.pending = None;
                self.activate_neon_item(item);
            } else {
                ui.ctx().request_repaint_after(ACTIVATE_DELAY.saturating_sub(now.duration_since(at)));
            }
        }
    }

    /// Leaving Main (or opening Settings over it): stop the renderer page so
    /// the next visit powers the signs on again.
    pub(in crate::app::egui_menu) fn release_neon_main_menu(&mut self) {
        self.neon_menu.pending = None;
        if self.neon_menu.sent.take().is_some() {
            self.render_command(RenderCommand::SetNeonMenu(None));
        }
    }

    fn activate_neon_item(&mut self, item: Item) {
        match item {
            Item::Play => {
                self.frontend_page = FrontendPage::Play;
                self.egui_repaint_requested = true;
            }
            Item::Profile => {
                self.open_profile_studio(None);
                self.frontend_page = FrontendPage::Profile;
                self.set_overlay(OverlayMode::Game);
            }
            Item::Controls => {
                self.frontend_page = FrontendPage::Controls;
                self.controls_waiting_for_key = false;
                self.egui_repaint_requested = true;
            }
            Item::Settings => {
                self.setup_selected = SETUP_TAB_VIDEO;
                self.set_overlay(OverlayMode::Video);
            }
            Item::ChatLogs => {
                self.frontend_page = FrontendPage::ChatLogs;
                self.egui_repaint_requested = true;
            }
            Item::Screenshots => {
                self.frontend_page = FrontendPage::Screenshots;
                self.screenshot_catalog_loaded = false;
                self.egui_repaint_requested = true;
            }
            Item::DeveloperTools => {
                self.frontend_page = FrontendPage::DeveloperTools;
                self.egui_repaint_requested = true;
            }
            Item::Quit => self.request_quit(),
        }
    }
}
