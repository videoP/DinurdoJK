//! Demo types.
use crate::app::{race_ghost, DemoConsoleKind, DemoMetadata, Instant, PlayerViewPolicyState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum SessionPhase {
    WaitingForMap,
    /// Live only: map loaded and usercmds flowing (CA_PRIMED), waiting for
    /// the first snapshot without SNAPFLAG_NOT_ACTIVE.
    WaitingForSnapshot,
    Playing,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::app) struct DemoCameraSample {
    pub(in crate::app) server_time: i32,
    /// High-resolution visual sample time. Protocol/event ownership still uses
    /// `server_time`; camera damping can use this without 1 ms quantization.
    pub(in crate::app) presentation_time_ms: f64,
    /// Native JKA origin for BSP cluster/area visibility tests.
    pub(in crate::app) native_origin: [f32; 3],
    /// Actor origin in native JKA coordinates. OpenJK third-person starts here.
    pub(in crate::app) eye_position: [f32; 3],
    pub(in crate::app) player_position: [f32; 3],
    pub(in crate::app) view_angles: [f32; 3],
    pub(in crate::app) velocity: [f32; 3],
    /// OpenJK playerState bobCycle; CG_CalculateWeaponPosition derives bob parity/phase from it.
    pub(in crate::app) bob_cycle: i32,
    pub(in crate::app) view_height: i32,
    pub(in crate::app) dead_yaw: f32,
    pub(in crate::app) client_num: i32,
    pub(in crate::app) policy: PlayerViewPolicyState,
    pub(in crate::app) following: bool,
    pub(in crate::app) teleported: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum DemoAdvance {
    Running,
    Completed,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::app) enum DemoTimelineHit {
    PlayPause,
    Track(f32),
    Speed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum DemoViewMode {
    Authoritative,
    Follow(i32),
    Free,
}

pub(in crate::app) const DEMO_FREE_SPEEDS: [f32; 7] =
    [78.75, 157.5, 315.0, 630.0, 1260.0, 2520.0, 5040.0];

pub(in crate::app) const DEMO_FREE_SPEED_DEFAULT_INDEX: usize = 3;

#[derive(Debug, Clone, Copy)]
pub(in crate::app) struct DemoFakeView {
    pub(in crate::app) client_num: i32,
    /// Native JKA actor origin; third-person collision starts here.
    pub(in crate::app) native_origin: [f32; 3],
    pub(in crate::app) eye_position: [f32; 3],
    pub(in crate::app) view_angles: [f32; 3],
    pub(in crate::app) velocity: [f32; 3],
    pub(in crate::app) health: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum DemoConsoleFilter {
    All,
    Print,
    Chat,
    CenterPrint,
    Event,
}

impl DemoConsoleFilter {
    pub(in crate::app) fn matches(self, kind: &DemoConsoleKind) -> bool {
        matches!(self, Self::All)
            || matches!(
                (self, *kind),
                (Self::Print, DemoConsoleKind::Print)
                    | (Self::Chat, DemoConsoleKind::Chat)
                    | (Self::CenterPrint, DemoConsoleKind::CenterPrint)
                    | (Self::Event, DemoConsoleKind::Event)
            )
    }
}

#[derive(Debug)]
pub(in crate::app) struct DemoMetadataResult {
    pub(in crate::app) generation: u64,
    pub(in crate::app) demo_name: String,
    pub(in crate::app) result: Result<DemoMetadata, String>,
}

pub(in crate::app) struct RaceGhostLoadResult {
    pub(in crate::app) generation: u64,
    pub(in crate::app) replace_existing: bool,
    pub(in crate::app) source_key: String,
    pub(in crate::app) result: Result<race_ghost::RaceGhostTrack, String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub(in crate::app) struct DemoEventStat {
    pub(in crate::app) count: u64,
    pub(in crate::app) handled: u64,
    pub(in crate::app) partial: u64,
    pub(in crate::app) unhandled: u64,
    pub(in crate::app) first_server_time: i32,
    pub(in crate::app) last_server_time: i32,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::app) struct DemoTimeline {
    pub(in crate::app) wall_anchor: Instant,
    pub(in crate::app) demo_anchor_ms: f64,
    pub(in crate::app) rate: f64,
    pub(in crate::app) resume_rate: f64,
}

impl DemoTimeline {
    pub(in crate::app) fn new(first_server_time: i32, now: Instant) -> Self {
        Self {
            wall_anchor: now,
            demo_anchor_ms: f64::from(first_server_time),
            rate: 1.0,
            resume_rate: 1.0,
        }
    }

    /// Continuous presentation time. Keep the demo clock at sub-millisecond
    /// precision even though JKA protocol/snapshot times are integer ms. This
    /// avoids quantizing multiple high-FPS render frames onto the same sample.
    pub(in crate::app) fn target_time_ms(self, now: Instant) -> f64 {
        let elapsed_ms = now
            .saturating_duration_since(self.wall_anchor)
            .as_secs_f64()
            * 1000.0;
        let value = self.demo_anchor_ms + elapsed_ms * self.rate;
        value.clamp(f64::from(i32::MIN), f64::from(i32::MAX))
    }

    pub(in crate::app) fn set_rate(&mut self, now: Instant, rate: f64) {
        // Preserve the exact fractional position when pausing or changing rate;
        // otherwise every control change can inject a <= 1 ms presentation hop.
        self.demo_anchor_ms = self.target_time_ms(now);
        self.wall_anchor = now;
        self.rate = rate;
        if rate > 0.0 {
            self.resume_rate = rate;
        }
    }

    pub(in crate::app) fn seek(&mut self, now: Instant, demo_time_ms: f64) {
        self.demo_anchor_ms = demo_time_ms.clamp(f64::from(i32::MIN), f64::from(i32::MAX));
        self.wall_anchor = now;
    }

    pub(in crate::app) fn toggle_pause(&mut self, now: Instant) {
        if self.rate == 0.0 {
            self.set_rate(now, self.resume_rate.max(0.01));
        } else {
            self.set_rate(now, 0.0);
        }
    }
}
