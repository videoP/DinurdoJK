//! Winit pointer/keyboard owner for the Taffy server-connection screen.
//! The exact same `ui::loading_layout` rectangles are used for painting.
use crate::app::App;
use crate::ui::loading_layout::{self, ConnectionAction};
use winit::{event::{ElementState, Ime, MouseButton, WindowEvent}, keyboard::{KeyCode, PhysicalKey}};

impl App {
    pub(in crate::app) fn native_connection_loading_event(&mut self, event: &WindowEvent) -> bool {
        let joining = (self.connection_modal_active() || (self.net.is_some() && self.loading.is_some()))
            && self.demo_missing_map_prompt.is_none();
        if !joining { return false; }
        // Leave the classic console usable during asynchronous map preparation.
        if self.overlay == crate::ui::OverlayMode::Console
            && self.server_password_prompt.is_none() && self.missing_map_prompt.is_none()
        { return false; }
        match event {
            WindowEvent::CursorMoved {position, ..} => {
                let next = [position.x as f32, position.y as f32];
                // Publish a new native UI snapshot only when the hovered Taffy
                // target changes. Moving within a button causes no extra UI
                // rebuild, but entering/leaving it changes its WGPU styling.
                let changed = if let (Some(window), Some(ui)) = (&self.window, self.unified_loading_ui()) {
                    let size = window.inner_size();
                    let geometry = loading_layout::layout(&ui, size.width, size.height);
                    let before = [self.cursor_position.0 as f32, self.cursor_position.1 as f32];
                    geometry.hit_test(before, ui.connection_prompt.as_ref())
                        != geometry.hit_test(next, ui.connection_prompt.as_ref())
                } else { false };
                self.cursor_position = (position.x, position.y);
                if changed { self.publish_ui(); }
                return true;
            }
            WindowEvent::CursorLeft { .. } => {
                let before = self.cursor_position;
                self.cursor_position = (-1.0, -1.0);
                if let (Some(window), Some(ui)) = (&self.window, self.unified_loading_ui()) {
                    let size = window.inner_size();
                    let geometry = loading_layout::layout(&ui, size.width, size.height);
                    if geometry.hit_test([before.0 as f32, before.1 as f32], ui.connection_prompt.as_ref()).is_some() {
                        self.publish_ui();
                    }
                }
                return true;
            }
            WindowEvent::MouseInput {state: ElementState::Pressed, button: MouseButton::Left, ..} => {
                let Some(window) = &self.window else {return true;};
                let size = window.inner_size();
                let Some(ui) = self.unified_loading_ui() else {return true;};
                let layout = loading_layout::layout(&ui,size.width,size.height);
                let p = [self.cursor_position.0 as f32,self.cursor_position.1 as f32];
                match layout.hit_test(p,ui.connection_prompt.as_ref()) {
                    Some(ConnectionAction::CancelJoin) => self.disconnect_to_main_menu(),
                    Some(ConnectionAction::Download) => self.choose_missing_map_autodownload(),
                    Some(ConnectionAction::ConnectAnyway) => self.choose_missing_map_connect_anyway(),
                    Some(ConnectionAction::Abort) => self.choose_missing_map_abort(),
                    Some(ConnectionAction::PasswordConnect) => self.submit_server_password_prompt(),
                    Some(ConnectionAction::PasswordCancel) => self.cancel_server_password_prompt(),
                    Some(ConnectionAction::PasswordField) | None => {},
                }
                return true;
            }
            WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. } => return true,
            WindowEvent::Ime(Ime::Commit(value)) if self.server_password_prompt.is_some() => {
                self.append_native_connection_password(value);
                return true;
            }
            WindowEvent::KeyboardInput {event: key, ..} => {
                if key.state == ElementState::Released { return true; }
                if let PhysicalKey::Code(code) = key.physical_key {
                    if code == KeyCode::Escape && !key.repeat {
                        if self.server_password_prompt.is_some() {
                            self.cancel_server_password_prompt();
                        } else if self.missing_map_prompt.is_some() {
                            self.choose_missing_map_abort();
                        } else { self.disconnect_to_main_menu(); }
                        return true;
                    }
                    if self.server_password_prompt.is_some() {
                        match code {
                            KeyCode::Enter | KeyCode::NumpadEnter if !key.repeat => {
                                self.submit_server_password_prompt(); return true;
                            }
                            KeyCode::Backspace => {
                                self.network.password.pop(); self.publish_ui(); return true;
                            }
                            KeyCode::KeyV if self.modifiers.control_key() && !key.repeat => {
                                if let Ok(text) = crate::clipboard::get_text() {
                                    self.append_native_connection_password(&text);
                                }
                                return true;
                            }
                            _ => {}
                        }
                        if !self.modifiers.control_key() && !self.modifiers.alt_key() {
                            if let Some(value) = key.text.as_ref() {
                                self.append_native_connection_password(value);
                            }
                        }
                    }
                }
                return true;
            }
            _ => {}
        }
        false
    }

    fn append_native_connection_password(&mut self, input: &str) {
        if self.server_password_prompt.is_none() {return;}
        // A server password is passed to the existing handshake. This only
        // replaces the text field; nothing in the authentication logic changes.
        let room = 128usize.saturating_sub(self.network.password.chars().count());
        if room == 0 {return;}
        self.network.password.extend(input.chars().filter(|c| !c.is_control()).take(room));
        self.publish_ui();
    }
}
