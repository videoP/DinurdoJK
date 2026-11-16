//! Windows Alt+numpad completion for the stock JKA byte-indexed editors.
//!
//! TaystJK processes SDL_TEXTINPUT after the OS translates Alt+numpad, then
//! converts each Unicode scalar through `ConvertUTF32ToExpectedCharset` before
//! queuing SE_CHAR. Winit 0.30's Win32 keyboard builder expects WM_CHAR to
//! follow a key-down, but Windows emits Alt+numpad text when Alt is released.
//! Capture only that completed native character with the existing message hook;
//! leave all other text, keyboard input, IMEs and gameplay keys to Winit.

use std::cell::RefCell;
use std::ffi::c_void;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MSG, WM_CHAR, WM_SYSCHAR, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_KEYDOWN, WM_KEYUP,
    WM_KILLFOCUS, WM_NCDESTROY,
};

#[derive(Default)]
struct AltCodeState {
    window: isize,
    enabled: bool,
    alt_down: bool,
    digits_seen: bool,
    awaiting_char: bool,
    completed: Vec<u16>,
}

thread_local! {
    static STATE: RefCell<AltCodeState> = RefCell::new(AltCodeState::default());
}

/// Only the main game HWND can emit text into chat/console. The companion
/// window and every unrelated external window are unaffected.
pub(crate) fn set_window(window: &winit::window::Window) {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = window.window_handle() else { return; };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else { return; };
    STATE.with_borrow_mut(|state| {
        state.window = handle.hwnd.get();
        state.alt_down = false;
        state.digits_seen = false;
        state.awaiting_char = false;
        state.completed.clear();
    });
}

pub(crate) fn set_enabled(enabled: bool) {
    STATE.with_borrow_mut(|state| {
        if state.enabled != enabled {
            state.enabled = enabled;
            state.alt_down = false;
            state.digits_seen = false;
            state.awaiting_char = false;
            state.completed.clear();
        }
    });
}

/// Called from Winit's already-installed Win32 event-loop message hook.
/// Return true ONLY for the completed Alt-code WM_CHAR/WM_SYSCHAR, so Winit
/// cannot double-insert it via a late/stray KeyEvent.text.
pub(crate) fn capture_message(message: *const c_void) -> bool {
    if message.is_null() { return false; }
    // SAFETY: Winit's message hook supplies a pointer to a valid MSG for the
    // duration of the callback; the value is read synchronously, never stored.
    let msg = unsafe { &*(message as *const MSG) };
    STATE.with_borrow_mut(|state| {
        if !state.enabled || state.window == 0 || msg.hwnd as isize != state.window {
            return false;
        }
        const VK_MENU: usize = 0x12;
        const VK_NUMPAD0: usize = 0x60;
        const VK_NUMPAD9: usize = 0x69;
        match msg.message {
            WM_SYSKEYDOWN | WM_KEYDOWN if msg.wParam == VK_MENU => {
                if !state.alt_down {
                    state.alt_down = true;
                    state.digits_seen = false;
                    state.awaiting_char = false;
                }
            }
            WM_SYSKEYDOWN | WM_KEYDOWN if state.alt_down => {
                if (VK_NUMPAD0..=VK_NUMPAD9).contains(&msg.wParam) {
                    state.digits_seen = true;
                } else {
                    // Alt+Enter, Alt+F4, AltGr, menu accelerators etc. are
                    // never mistaken for a Windows decimal Alt-code sequence.
                    state.digits_seen = false;
                }
            }
            WM_SYSKEYUP | WM_KEYUP if msg.wParam == VK_MENU => {
                state.awaiting_char = state.alt_down && state.digits_seen;
                state.alt_down = false;
                state.digits_seen = false;
            }
            WM_CHAR | WM_SYSCHAR if state.awaiting_char || (state.alt_down && state.digits_seen) => {
                state.awaiting_char = false;
                state.alt_down = false;
                state.digits_seen = false;
                state.completed.push(msg.wParam as u16);
                return true;
            }
            WM_SYSKEYDOWN | WM_KEYDOWN => {
                // If the OS didn't emit a character, don't intercept the
                // following unrelated typed character.
                state.awaiting_char = false;
            }
            WM_KILLFOCUS | WM_NCDESTROY => {
                state.alt_down = false;
                state.digits_seen = false;
                state.awaiting_char = false;
            }
            _ => {}
        }
        false
    })
}

/// Exact TaystJK `ConvertUTF32ToExpectedCharset` semantics, scoped to Alt
/// input only. The regular Unicode chat editor (and protocol decoding) must
/// not be reinterpreted as CP437 or forced into Windows-1251/1250.
fn taystjk_alt_byte(ch: char) -> u8 {
    let c = ch as u32;
    match c {
        0x0410..=0x044F => (c - 0x0410 + 192) as u8,
        0x0160 => 138, 0x015A => 140, 0x0164 => 141, 0x017D => 142, 0x0179 => 143,
        0x0161 => 154, 0x015B => 156, 0x0165 => 157, 0x017E => 158, 0x017A => 159,
        0x0141 => 163, 0x0104 => 165, 0x015E => 170, 0x017B => 175,
        0x0142 => 179, 0x0105 => 185, 0x015F => 186, 0x013D => 188,
        0x013E => 190, 0x017C => 191,
        0x0154 => 192, 0x00C1 => 193, 0x00C2 => 194, 0x0102 => 195,
        0x00C4 => 196, 0x0139 => 197, 0x0106 => 198, 0x00C7 => 199,
        0x010C => 200, 0x00C9 => 201, 0x0118 => 202, 0x00CB => 203,
        0x011A => 204, 0x00CD => 205, 0x00CE => 206, 0x010E => 207,
        0x0110 => 208, 0x0143 => 209, 0x0147 => 210, 0x00D3 => 211,
        0x00D4 => 212, 0x0150 => 213, 0x00D6 => 214, 0x0158 => 216,
        0x016E => 217, 0x00DA => 218, 0x0170 => 219, 0x00DC => 220,
        0x00DD => 221, 0x0162 => 222, 0x00DF => 223,
        0x0155 => 224, 0x00E1 => 225, 0x00E2 => 226, 0x0103 => 227,
        0x00E4 => 228, 0x013A => 229, 0x0107 => 230, 0x00E7 => 231,
        0x010D => 232, 0x00E9 => 233, 0x0119 => 234, 0x00EB => 235,
        0x011B => 236, 0x00ED => 237, 0x00EE => 238, 0x010F => 239,
        0x0111 => 240, 0x0144 => 241, 0x0148 => 242, 0x00F3 => 243,
        0x00F4 => 244, 0x0151 => 245, 0x00F6 => 246, 0x0159 => 248,
        0x016F => 249, 0x00FA => 250, 0x0171 => 251, 0x00FC => 252,
        0x00FD => 253, 0x0163 => 254, 0x02D9 => 255,
        _ => c as u8, // TaystJK's uint8_t fallback, including U+2310 -> 0x10
    }
}

pub(crate) fn take_completed() -> Vec<u8> {
    STATE.with_borrow_mut(|state| {
        state.completed.drain(..)
            .filter_map(|unit| char::from_u32(unit as u32))
            .map(taystjk_alt_byte)
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taystjk_alt_codes_are_not_cp437_reverse_mapping() {
        // No leading zero: on a CP437 OEM Windows setup, Alt+169 -> U+2310.
        assert_eq!(taystjk_alt_byte('⌐'), 0x10);
        // Leading zero: Windows ANSI Alt+0169 -> U+00A9.
        assert_eq!(taystjk_alt_byte('©'), 0xA9);
        assert_eq!(taystjk_alt_byte('Ж'), 0xC6);
        assert_eq!(taystjk_alt_byte('Š'), 138);
        assert_eq!(taystjk_alt_byte('A'), b'A');
        // No change to ordinary Unicode chat or pasted characters.
        assert_eq!(crate::cgame::unicode_chat_wire_bytes("⌐©Ж"), "⌐".as_bytes().iter().copied()
            .chain([0xA9]).chain("Ж".as_bytes().iter().copied()).collect::<Vec<_>>());
    }
}
