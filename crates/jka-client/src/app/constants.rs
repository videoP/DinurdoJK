//! Constants.
use crate::app::Duration;

pub(in crate::app) const FALLBACK_RESOLUTIONS: [[u32; 2]; 5] = [
    [1280, 800],
    [1600, 900],
    [1920, 1080],
    [2560, 1440],
    [3840, 2160],
];

pub(in crate::app) const VIDEO_CONFIRM_TIMEOUT_SECS: u64 = 15;

pub(in crate::app) const DEMO_SCRUB_PREVIEW_INTERVAL: Duration = Duration::from_micros(41_667);
// 24 Hz

/// The frontend is a real rendered world, not a 2D levelshot. Keep the map
/// fixed for now so menu art direction is deterministic; later this can become
/// a cvar or an authored list of menu scenes without changing the load path.
pub(in crate::app) const FRONTEND_BACKGROUND_MAP: &str = "mp/duel3";

pub(in crate::app) const FRONTEND_CAMERA_EYE_HEIGHT: f32 = 18.0;

pub(in crate::app) const FRONTEND_FOREGROUND_MODEL: &str = "jedi_hm/head_a1|torso_d1|lower_a1";

pub(in crate::app) const FRONTEND_FOREGROUND_MODEL_DISTANCE: f32 = 52.0;

pub(in crate::app) const FRONTEND_FOREGROUND_MODEL_SIDE_OFFSET: f32 = 46.0;

pub(in crate::app) const FRONTEND_FOREGROUND_MODEL_HEAD_HEIGHT: f32 = 44.0;
