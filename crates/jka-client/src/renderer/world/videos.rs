//! world videos.

/// One looping `videoMap` cinematic. RoQ frames are deltas, so every frame must be
/// decoded in order; only the newest decoded frame of a tick is uploaded.
pub(in crate::renderer) struct WorldVideoPlayer {
    pub(in crate::renderer) texture: usize,
    pub(in crate::renderer) video: jka_assets::roq::RoqVideo,
    pub(in crate::renderer) next_frame_at: f64,
}

/// Most frames decoded in one render call before the player resynchronises
/// to the clock (after a stall or a long load).
pub(in crate::renderer) const WORLD_VIDEO_MAX_CATCH_UP: u32 = 8;
