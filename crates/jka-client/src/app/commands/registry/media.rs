//! Declarative command handlers.
use crate::app::{App, Instant};

use super::{CommandDef, CommandResult, CommandScope};
commands! {
    cmd_cameraedit ["cameraedit", ] ("cameraedit", "Adjust the third-person camera with the mouse: scroll to zoom, drag to move, hold right mouse to aim.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.start_camera_edit();
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_record ["record", ] ("record [demoname]", "Start recording the current live session to a native demos/*.dm_26 file.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb, demo_name] => {
                self.start_demo_recording(Some(demo_name));
                CommandResult::Handled
            }
            [_verb] => {
                self.start_demo_recording(None);
                CommandResult::Handled
            }
            [_verb, ..] => {
                self.push_console_line("^3record [demoname]".to_owned());
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_stoprecord ["stoprecord", ] ("stoprecord", "Stop the current demo recording and write its end marker.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.stop_demo_recording();
                CommandResult::Handled
            }
            [_verb, ..] => {
                self.push_console_line("^3stoprecord".to_owned());
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_demo ["demo", ] ("demo <demoname>", "Play demos/<demoname>.dm_26 using the same playback path as the GUI.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb, demo_name] => {
                self.play_demo_named(demo_name);
                CommandResult::Handled
            }
            [_verb] => {
                self.push_console_line("^3demo <demoname>".to_owned());
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_rghost ["rGhost", ] ("rGhost <demoname|clear>", "Load a demos/<demoname>.dm_26 race run as a visual-only ghost synchronized to your duelTime. Use rGhost clear to remove it.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb, action] if action.eq_ignore_ascii_case("clear") => {
                self.clear_race_ghosts();
                CommandResult::Handled
            }
            [_verb, demo_name] => {
                self.load_race_ghost_named(demo_name);
                CommandResult::Handled
            }
            [_verb] => {
                self.push_console_line("^3rGhost <demoname>^7 | ^3rGhost clear".to_owned());
                CommandResult::Handled
            }
            [_verb, ..] => {
                self.push_console_line("^3rGhost <demoname>^7 | ^3rGhost clear".to_owned());
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_screenshot ["screenshot", ] ("screenshot", "Save the next completed frame under the active fs_game/screenshots directory (base/screenshots when no mod is active).", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.request_screenshot();
                self.console_status = "SCREENSHOT QUEUED".into();
                CommandResult::Status
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_screenshot_png ["screenshot_png", ] ("screenshot_png", "Save a PNG screenshot with embedded DinurdoJK metadata.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.request_screenshot_png();
                self.console_status = "PNG SCREENSHOT QUEUED".into();
                CommandResult::Status
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_demo_pause ["demo_pause", ] ("demo_pause", "Pause or resume demo playback.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.console_status = match self.game_session.as_mut() {
                    Some(playback) => match playback.toggle_pause(Instant::now()) {
                        Ok(0.0) => "DEMO PAUSED".into(),
                        Ok(rate) => format!("DEMO PLAYING AT {rate}X"),
                        Err(error) => error,
                    },
                    None => "NO DEMO IS PLAYING".into(),
                };
                CommandResult::Status
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_demo_seek ["demo_seek", ] ("demo_seek <seconds>", "Seek demo playback to <seconds> after the demo's first active snapshot.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb, seconds] => {
                let Some(target_ms) = seconds.parse::<f64>().ok().filter(|s| s.is_finite() && *s >= 0.0)
                    .map(|s| (s * 1000.0).round() as i32) else {
                    self.console_status = "USAGE: demo_seek <seconds>".into();
                    return CommandResult::Status;
                };
                let current = self.game_session.as_ref().filter(|session| !session.live)
                    .and_then(|session| session.demo_elapsed_ms(Instant::now()));
                match current {
                    Some(current) => {
                        self.seek_demo_relative(target_ms.saturating_sub(current.round() as i32));
                        self.push_console_line(format!("DEMO SEEK: {:.3}s", f64::from(target_ms) / 1000.0));
                    }
                    None => self.push_console_line("demo_seek: no demo is playing".to_owned()),
                }
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_demo_speed ["demo_speed", ] ("demo_speed <0..100>", "Set demo playback speed.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb, rate] => {
                self.console_status = match rate.parse::<f64>() {
                    Ok(rate) => match self.game_session.as_mut() {
                        Some(playback) => match playback.set_playback_rate(Instant::now(), rate) {
                            Ok(()) if rate == 0.0 => "DEMO PAUSED".into(),
                            Ok(()) => format!("DEMO SPEED: {rate}X"),
                            Err(error) => error,
                        },
                        None => "NO DEMO IS PLAYING".into(),
                    },
                    Err(_) => "USAGE: demo_speed <0..100>".into(),
                };
                CommandResult::Status
            }
            _ => return CommandResult::Unhandled,
        }
    }

}
