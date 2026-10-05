//! Demo controls.
use crate::app::{
    ui, App, CenterPrintRecord, ChatRecord, DemoConsoleKind, DemoKillMarkerUi, DemoTimelineHit,
    DemoTimelineUi, DemoViewMode, Duration, Instant, OverlayMode, SessionPhase,
    ThirdPersonCameraState, UiChatLine, DEMO_FREE_SPEEDS, DEMO_SCRUB_PREVIEW_INTERVAL,
};

impl App {
    pub(in crate::app) fn current_demo_elapsed_ms(&self, now: Instant) -> Option<f64> {
        self.game_session
            .as_ref()
            .filter(|session| !session.live)
            .and_then(|session| session.demo_elapsed_ms(now))
    }

    pub(in crate::app) fn transient_ui_snapshot(
        &self,
    ) -> (Vec<UiChatLine>, Option<ui::UiCenterPrint>) {
        let now = Instant::now();
        let demo_now = self.current_demo_elapsed_ms(now);
        let chat_lines = self
            .chat_lines
            .iter()
            .filter_map(|line| {
                let age_ms = match (demo_now, line.demo_elapsed_ms) {
                    (Some(now_ms), Some(created_ms)) => (now_ms - created_ms).max(0.0),
                    _ => now.saturating_duration_since(line.created).as_secs_f64() * 1000.0,
                };
                // Stock MP cg_chatBox defaults to 10000 ms. Demo messages age
                // against demo time, so pause/scrub is deterministic and cannot flicker.
                if age_ms >= 10_000.0 {
                    return None;
                }
                let alpha = if age_ms <= 9_000.0 {
                    1.0
                } else {
                    1.0 - ((age_ms - 9_000.0) / 1_000.0) as f32
                };
                Some(UiChatLine {
                    text: line.text.clone(),
                    alpha: alpha.clamp(0.0, 1.0),
                })
            })
            .collect();

        let center_print = self.center_print.as_ref().and_then(|print| {
            const CENTER_TIME: Duration = Duration::from_secs(3);
            const FADE_TIME: Duration = Duration::from_millis(200);
            let age = match (demo_now, print.demo_elapsed_ms) {
                (Some(now_ms), Some(created_ms)) => {
                    Duration::from_secs_f64(((now_ms - created_ms).max(0.0)) / 1000.0)
                }
                _ => now.saturating_duration_since(print.created),
            };
            if age >= CENTER_TIME {
                return None;
            }
            let remaining = CENTER_TIME.saturating_sub(age);
            let alpha = if remaining < FADE_TIME {
                remaining.as_secs_f32() / FADE_TIME.as_secs_f32()
            } else {
                1.0
            };
            Some(ui::UiCenterPrint {
                text: print.text.clone(),
                alpha,
                y_fraction: print.y_fraction,
            })
        });

        (chat_lines, center_print)
    }

    pub(in crate::app) fn restore_demo_transients(&mut self, elapsed_ms: i32, now: Instant) {
        let notices = self
            .game_session
            .as_ref()
            .and_then(|session| session.demo_index.as_ref())
            .map(|index| index.transient_notices.clone())
            .unwrap_or_default();
        self.chat_lines.clear();
        self.center_print = None;
        self.last_center_print.clear();

        let chat_start = elapsed_ms.saturating_sub(10_000);
        for entry in notices
            .iter()
            .filter(|entry| {
                entry.kind == DemoConsoleKind::Chat
                    && entry.elapsed_ms >= chat_start
                    && entry.elapsed_ms <= elapsed_ms
            })
            .rev()
            .take(16)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            self.chat_lines.push_back(ChatRecord {
                text: entry.text.clone(),
                created: now,
                demo_elapsed_ms: Some(f64::from(entry.elapsed_ms)),
            });
        }
        if let Some(entry) = notices.iter().rev().find(|entry| {
            entry.kind == DemoConsoleKind::CenterPrint
                && entry.elapsed_ms <= elapsed_ms
                && elapsed_ms.saturating_sub(entry.elapsed_ms) < 3_000
        }) {
            let rendered = self.translate_server_text(entry.text.as_bytes());
            self.center_print = Some(CenterPrintRecord {
                text: rendered,
                created: now,
                demo_elapsed_ms: Some(f64::from(entry.elapsed_ms)),
                y_fraction: 0.30,
            });
        }
        self.last_chat_refresh = now;
        self.last_center_refresh = now;
    }

    pub(in crate::app) fn demo_timeline_ui(&self, now: Instant) -> Option<DemoTimelineUi> {
        let session = self
            .game_session
            .as_ref()
            .filter(|session| !session.live && session.phase == SessionPhase::Playing)?;
        let index = session.demo_index.as_ref()?;
        let first = index.first_active_server_time?;
        let duration_ms = index.duration_ms();
        if duration_ms <= 0 {
            return None;
        }
        let authoritative = session.demo_followed_client().unwrap_or(-1);
        let followed = match self.demo_view_mode {
            DemoViewMode::Follow(client) => client,
            _ => authoritative,
        };
        let duration = duration_ms as f32;
        let kill_markers = index
            .kill_markers
            .iter()
            .map(|marker| DemoKillMarkerUi {
                fraction: (marker.server_time.saturating_sub(first) as f32 / duration)
                    .clamp(0.0, 1.0),
                attacker_team: marker.attacker_team,
                followed_kill: marker.attacker == followed && marker.target != followed,
                followed_death: marker.target == followed,
            })
            .collect();
        let (paused, playback_rate) = session
            .timeline
            .map(|timeline| {
                let paused = timeline.rate == 0.0;
                let speed = if paused {
                    timeline.resume_rate
                } else {
                    timeline.rate
                };
                (paused, speed)
            })
            .unwrap_or((true, 1.0));
        let camera_label = match self.demo_view_mode {
            DemoViewMode::Authoritative => session
                .demo_visible_players()
                .into_iter()
                .find(|(client, _)| *client == authoritative)
                .map(|(_, name)| format!("RECORDED: {name}"))
                .unwrap_or_else(|| "RECORDED POV".to_owned()),
            DemoViewMode::Follow(client) => session
                .demo_visible_players()
                .into_iter()
                .find(|(candidate, _)| *candidate == client)
                .map(|(_, name)| format!("FAKE POV: {name}"))
                .unwrap_or_else(|| format!("FAKE POV: CLIENT {client}")),
            DemoViewMode::Free => format!(
                "FREE VIEW · {:.0} U/S",
                DEMO_FREE_SPEEDS[self.demo_free_speed_index.min(DEMO_FREE_SPEEDS.len() - 1)]
            ),
        };
        Some(DemoTimelineUi {
            elapsed_ms: session.demo_elapsed_ms(now).unwrap_or(0.0),
            duration_ms,
            playback_rate,
            paused,
            scrub_fraction: self.demo_scrub_fraction,
            kill_markers,
            camera_label,
            outside_authoritative_view: self.demo_view_outside_authoritative,
        })
    }

    pub(in crate::app) fn demo_timeline_hit(&self, x: f64, y: f64) -> Option<DemoTimelineHit> {
        if !self.demo_playback_active() || self.overlay != OverlayMode::None {
            return None;
        }
        let size = self.window.as_ref()?.inner_size();
        let layout = ui::demo_timeline_layout(size.width, size.height)?;
        let point = [x as f32, y as f32];
        let inside = |rect: [f32; 4]| {
            point[0] >= rect[0]
                && point[0] <= rect[0] + rect[2]
                && point[1] >= rect[1]
                && point[1] <= rect[1] + rect[3]
        };
        // During demo playback the mouse belongs to the camera by default.
        // Holding the user's +speed bind is the explicit, temporary UI modifier.
        if !self.live_buttons.contains("+speed") {
            return None;
        }
        if inside(layout.play) {
            return Some(DemoTimelineHit::PlayPause);
        }
        if inside(layout.speed) {
            return Some(DemoTimelineHit::Speed);
        }
        if inside(layout.track) {
            let fraction = ((point[0] - layout.track[0]) / layout.track[2]).clamp(0.0, 1.0);
            return Some(DemoTimelineHit::Track(fraction));
        }
        None
    }

    pub(in crate::app) fn toggle_demo_pause_ui(&mut self) {
        let status = self.game_session.as_mut().and_then(|session| {
            (!session.live)
                .then(|| session.toggle_pause(Instant::now()))
                .and_then(Result::ok)
        });
        if let Some(rate) = status {
            self.console_status = if rate == 0.0 {
                "DEMO PAUSED".into()
            } else {
                format!("DEMO PLAYING AT {rate}X")
            };
            self.publish_transient_ui();
        }
    }

    pub(in crate::app) fn cycle_demo_speed_ui(&mut self) {
        const RATES: [f64; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];
        let Some(session) = self.game_session.as_mut().filter(|session| !session.live) else {
            return;
        };
        let current = session
            .timeline
            .map(|timeline| {
                if timeline.rate > 0.0 {
                    timeline.rate
                } else {
                    timeline.resume_rate
                }
            })
            .unwrap_or(1.0);
        let next = RATES
            .iter()
            .copied()
            .find(|rate| *rate > current + 0.001)
            .unwrap_or(RATES[0]);
        if session.set_playback_speed(Instant::now(), next).is_ok() {
            self.console_status = format!("DEMO SPEED: {next}X");
            self.publish_transient_ui();
        }
    }

    pub(in crate::app) fn lower_demo_speed_ui(&mut self) {
        const RATES: [f64; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];
        let Some(session) = self.game_session.as_mut().filter(|session| !session.live) else {
            return;
        };
        let current = session
            .timeline
            .map(|timeline| {
                if timeline.rate > 0.0 {
                    timeline.rate
                } else {
                    timeline.resume_rate
                }
            })
            .unwrap_or(1.0);
        let next = RATES
            .iter()
            .copied()
            .rev()
            .find(|rate| *rate < current - 0.001)
            .unwrap_or(*RATES.last().unwrap());
        if session.set_playback_speed(Instant::now(), next).is_ok() {
            self.console_status = format!("DEMO SPEED: {next}X");
            self.publish_transient_ui();
        }
    }

    pub(in crate::app) fn seek_demo_scrub_fraction(
        &mut self,
        fraction: f32,
        now: Instant,
    ) -> Result<(), String> {
        let (sample, elapsed) = {
            let session = self
                .game_session
                .as_mut()
                .filter(|session| !session.live)
                .ok_or_else(|| "NO DEMO SESSION".to_owned())?;
            let duration = session
                .demo_duration_ms()
                .ok_or_else(|| "DEMO TIMELINE IS NOT READY".to_owned())?;
            let elapsed = (duration as f32 * fraction.clamp(0.0, 1.0)).round() as i32;
            (session.seek_demo(now, elapsed)?, elapsed)
        };
        self.restore_demo_transients(elapsed, now);
        self.third_person_camera = ThirdPersonCameraState::default();
        self.apply_demo_view(sample, Duration::ZERO);
        self.previous_tick = now;
        Ok(())
    }

    pub(in crate::app) fn begin_demo_scrub(&mut self, fraction: f32) {
        let Some(session) = self.game_session.as_mut().filter(|session| !session.live) else {
            return;
        };
        self.demo_scrub_resume_rate = session.playback_rate().unwrap_or(0.0);
        let now = Instant::now();
        session.set_audio_suppressed(true);
        let _ = session.set_playback_rate(now, 0.0);
        self.demo_scrub_dragging = true;
        let fraction = fraction.clamp(0.0, 1.0);
        self.demo_scrub_fraction = Some(fraction);
        self.demo_scrub_last_seek = Some(now);
        match self.seek_demo_scrub_fraction(fraction, now) {
            Ok(()) => self.demo_scrub_last_applied_fraction = Some(fraction),
            Err(error) => {
                self.console_status = format!("DEMO SEEK ERROR: {error}");
                self.push_console_line(format!("^1{}", self.console_status));
            }
        }
        self.publish_transient_ui();
    }

    pub(in crate::app) fn update_demo_scrub(&mut self, x: f64) {
        if !self.demo_scrub_dragging {
            return;
        }
        let Some(size) = self.window.as_ref().map(|window| window.inner_size()) else {
            return;
        };
        let Some(layout) = ui::demo_timeline_layout(size.width, size.height) else {
            return;
        };
        let fraction = ((x as f32 - layout.track[0]) / layout.track[2]).clamp(0.0, 1.0);
        self.demo_scrub_fraction = Some(fraction);

        // The real seek happens from `tick_demo_scrub_preview`: winit can deliver
        // many CursorMoved events in one turn, so keeping only this latest target
        // truly coalesces them instead of replaying stale intermediate positions.
        self.publish_transient_ui();
    }

    pub(in crate::app) fn tick_demo_scrub_preview(&mut self, now: Instant) {
        if !self.demo_scrub_dragging {
            return;
        }
        let Some(fraction) = self.demo_scrub_fraction else {
            return;
        };
        if self
            .demo_scrub_last_applied_fraction
            .is_some_and(|applied| (applied - fraction).abs() <= f32::EPSILON)
        {
            return;
        }
        let due = self
            .demo_scrub_last_seek
            .is_none_or(|last| now.saturating_duration_since(last) >= DEMO_SCRUB_PREVIEW_INTERVAL);
        if !due {
            return;
        }
        self.demo_scrub_last_seek = Some(now);
        match self.seek_demo_scrub_fraction(fraction, now) {
            Ok(()) => self.demo_scrub_last_applied_fraction = Some(fraction),
            Err(error) => {
                self.console_status = format!("DEMO SEEK ERROR: {error}");
                self.push_console_line(format!("^1{}", self.console_status));
            }
        }
    }

    pub(in crate::app) fn finish_demo_scrub(&mut self) {
        if !self.demo_scrub_dragging {
            return;
        }
        self.demo_scrub_dragging = false;
        let fraction = self
            .demo_scrub_fraction
            .take()
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        self.demo_scrub_last_seek = None;
        self.demo_scrub_last_applied_fraction = None;
        let now = Instant::now();
        let seek_result = self.seek_demo_scrub_fraction(fraction, now);
        if let Some(session) = self.game_session.as_mut().filter(|session| !session.live) {
            // The exact release seek is silent too. Only after we are settled at
            // the final destination do we allow audio presentation again.
            session.set_audio_suppressed(false);
            if seek_result.is_ok() && self.demo_scrub_resume_rate > 0.0 {
                let _ = session.set_playback_rate(now, self.demo_scrub_resume_rate);
            }
        }
        if let Err(error) = seek_result {
            self.console_status = format!("DEMO SEEK ERROR: {error}");
            self.push_console_line(format!("^1{}", self.console_status));
        }
        self.publish_transient_ui();
        // If +speed was released before the mouse button, the scrub itself kept
        // cursor ownership. Mouse-up ends that ownership and may recapture now.
        self.sync_demo_camera_capture();
    }

    pub(in crate::app) fn seek_demo_relative(&mut self, delta_ms: i32) {
        let now = Instant::now();
        let result = self.game_session.as_mut().and_then(|session| {
            if session.live {
                return None;
            }
            let current = session.demo_elapsed_ms(now)?.round() as i32;
            Some(session.seek_demo(now, current.saturating_add(delta_ms)))
        });
        if let Some(Ok(sample)) = result {
            let elapsed = self
                .game_session
                .as_ref()
                .and_then(|session| session.demo_elapsed_ms(now))
                .unwrap_or(0.0)
                .round() as i32;
            self.restore_demo_transients(elapsed, now);
            self.third_person_camera = ThirdPersonCameraState::default();
            self.apply_demo_view(sample, Duration::ZERO);
            self.previous_tick = now;
            self.publish_transient_ui();
        } else if let Some(Err(error)) = result {
            self.console_status = format!("DEMO SEEK ERROR: {error}");
            self.push_console_line(format!("^1{}", self.console_status));
        }
    }
}
