//! Public boundary types for the shared actor water wake. One host track
//! builder (jka-river `river_ninja::append_wake_tracks`) feeds the river,
//! Ocean2 and Coast fields; the private side converts every track with the
//! same stroke builder. Plain data only.

/// Maximum host records: the JKA entity namespace plus 64 players' extra joints.
pub const WAKE_MAX_TRACKS: usize = 1024 + 64 * 12;

/// Geometry supplied by the host; source-specific brush sizing stays private.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WakeTrackSource {
    Skeletal { parent_distance_cm: f32 },
    Primitive { bounds_radius_cm: f32 },
}

/// One tracked point in field-relative centimetres (z up). Stable owner/slot
/// keys identify the same point across fixed simulation ticks. A missing
/// previous point means the host observed a discontinuity or newly acquired
/// point. Order must match the frame's contact flags.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WakeTrack {
    pub actor_id: u16,
    pub slot: u8,
    pub continuity: i32,
    pub time_ms: i32,
    pub origin_cm: [f32; 3],
    pub previous_cm: Option<[f32; 3]>,
    pub source: WakeTrackSource,
}
