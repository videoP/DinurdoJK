//! Clock for the docked chat field: open/close easing, the "still typing"
//! dots, the caret blink, and the combat mute for chat sounds. Everything is
//! derived from a few instants, so the renderer only needs a fresh
//! `UiChatAnim` while something is moving.
use crate::app::{App, Duration, Instant, OverlayMode};
use crate::ui::UiChatAnim;

/// Chat lines retained for the typing view (`cg_chatHistoryTime` trims them).
pub(in crate::app) const KEEP_LINES: usize = 64;
const OPEN_MS: f32 = 160.0;
const CLOSE_MS: f32 = 140.0;

pub(in crate::app) struct State {
    was_open: bool,
    changed_at: Instant,
    last_input: String,
    last_key: Instant,
    last_attack: Option<Instant>,
    last_publish: Instant,
}

impl Default for State {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            was_open: false,
            changed_at: now - Duration::from_secs(10),
            last_input: String::new(),
            last_key: now,
            last_attack: None,
            last_publish: now,
        }
    }
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t)
}

impl App {
    pub(in crate::app) fn chat_anim_ui(&self, now: Instant) -> UiChatAnim {
        let state = &self.chat_anim;
        let since = now.saturating_duration_since(state.changed_at).as_secs_f32() * 1000.0;
        let open = if state.was_open {
            ease_out(since / OPEN_MS)
        } else {
            1.0 - ease_out(since / CLOSE_MS)
        };
        let idle_ms = now.saturating_duration_since(state.last_key).as_millis().min(u128::from(u32::MAX)) as u32;
        UiChatAnim { open, idle_ms, caret: (idle_ms / 530) % 2 == 0 }
    }

    /// True while chat sounds are held back because we just attacked.
    pub(in crate::app) fn chat_sounds_muted_by_combat(&self) -> bool {
        let window = self.chat_settings.mute_after_attack;
        window > 0.0
            && self
                .chat_anim
                .last_attack
                .is_some_and(|at| at.elapsed().as_secs_f32() < window)
    }

    /// Track open/close, typing and attacks. True when the field is moving and
    /// the transient UI should be republished (about 30 times a second).
    pub(in crate::app) fn tick_chat_anim(&mut self, now: Instant) -> bool {
        if self.live_buttons.contains("+attack") || self.live_buttons.contains("+altattack") {
            self.chat_anim.last_attack = Some(now);
        }
        let open = self.overlay == OverlayMode::Chat;
        let state = &mut self.chat_anim;
        if open != state.was_open {
            state.was_open = open;
            state.changed_at = now;
            state.last_key = now;
        }
        if state.last_input != self.chat_input {
            state.last_input.clone_from(&self.chat_input);
            state.last_key = now;
        }
        let settling = now.saturating_duration_since(state.changed_at).as_secs_f32() * 1000.0 < OPEN_MS.max(CLOSE_MS) + 40.0;
        let animating = self.chat_settings.docked && (open || settling);
        if animating && now.saturating_duration_since(state.last_publish) >= Duration::from_millis(33) {
            state.last_publish = now;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::ease_out;

    #[test]
    fn easing_runs_from_zero_to_one() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        assert!(ease_out(0.5) > 0.5);
        assert_eq!(ease_out(3.0), 1.0);
    }
}
