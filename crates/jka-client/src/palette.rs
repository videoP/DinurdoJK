//! The client's one colour palette.
//!
//! Five base tones, shared by the egui menus (`app::egui_theme`) and the
//! renderer-native gameplay UI (`ui::*`), so a menu, the scoreboard and the
//! kill feed read as one family:
//!
//! | name        | hex      | role                                  |
//! |-------------|----------|---------------------------------------|
//! | Celadon     | `afd5aa` | accent, selection, positive state     |
//! | White Smoke | `f0f2ef` | primary text                          |
//! | Stone Brown | `5c5346` | structure: rules, outlines, surfaces  |
//! | Silver      | `a69f98` | secondary text                        |
//! | Smoky Rose  | `8c6057` | warm secondary accent, danger         |
//!
//! Everything else here is a shade of one of those five, mixed toward the
//! near-black Stone Brown base so dark surfaces keep the warm undertone.

pub(crate) const CELADON: [u8; 3] = [0xAF, 0xD5, 0xAA];
pub(crate) const WHITE_SMOKE: [u8; 3] = [0xF0, 0xF2, 0xEF];
pub(crate) const STONE_BROWN: [u8; 3] = [0x5C, 0x53, 0x46];
pub(crate) const SILVER: [u8; 3] = [0xA6, 0x9F, 0x98];
pub(crate) const SMOKY_ROSE: [u8; 3] = [0x8C, 0x60, 0x57];

/// Darkest surface: Stone Brown at roughly a fifth of its brightness.
pub(crate) const INK: [u8; 3] = [0x14, 0x12, 0x10];

/// Linear mix of two palette tones, `t` = 0 gives `a`.
pub(crate) const fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)]
}

const fn lerp(x: u8, y: u8, t: f32) -> u8 {
    (x as f32 + (y as f32 - x as f32) * t + 0.5) as u8
}

/// Palette tone as the `[r, g, b, a]` floats the 2D UI vertices take.
pub(crate) const fn rgba(c: [u8; 3], alpha: f32) -> [f32; 4] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, alpha]
}

/// Gameplay-UI shades. The scoreboard, kill feed and vote menu draw with
/// these instead of ad-hoc literals.
pub(crate) mod hud {
    use super::*;

    pub(crate) const TEXT: [f32; 4] = rgba(WHITE_SMOKE, 1.0);
    pub(crate) const TEXT_DIM: [f32; 4] = rgba(SILVER, 1.0);
    pub(crate) const TEXT_FAINT: [f32; 4] = rgba(mix(SILVER, STONE_BROWN, 0.45), 1.0);
    pub(crate) const ACCENT: [f32; 4] = rgba(CELADON, 1.0);
    pub(crate) const ROSE: [f32; 4] = rgba(SMOKY_ROSE, 1.0);
    pub(crate) const ROSE_LIGHT: [f32; 4] = rgba(mix(SMOKY_ROSE, WHITE_SMOKE, 0.35), 1.0);
    pub(crate) const RULE: [f32; 4] = rgba(STONE_BROWN, 1.0);

    /// Header/footer slab: opaque enough to hold text over any scene.
    pub(crate) const SLAB_TOP: [f32; 4] = rgba(mix(INK, STONE_BROWN, 0.30), 0.94);
    pub(crate) const SLAB_BOTTOM: [f32; 4] = rgba(INK, 0.94);
    /// Translucent strip behind a single HUD line (kill feed, menus).
    pub(crate) const STRIP: [f32; 4] = rgba(INK, 0.62);
    pub(crate) const SHADOW: [f32; 4] = [0.0, 0.0, 0.0, 0.32];

    /// Team colours, kept recognisable as red/blue but pulled toward the
    /// palette's warm, muted register.
    pub(crate) const TEAM_RED: [f32; 4] = rgba(mix(SMOKY_ROSE, [0xE0, 0x4A, 0x3E], 0.6), 1.0);
    pub(crate) const TEAM_BLUE: [f32; 4] = rgba(mix([0x5A, 0x8C, 0xD8], WHITE_SMOKE, 0.12), 1.0);
    pub(crate) const GOLD: [f32; 4] = rgba([0xE6, 0xC2, 0x6E], 1.0);

    /// `color` with its alpha multiplied by `alpha`.
    pub(crate) fn fade(color: [f32; 4], alpha: f32) -> [f32; 4] {
        [color[0], color[1], color[2], color[3] * alpha]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_hits_both_ends() {
        assert_eq!(mix(CELADON, INK, 0.0), CELADON);
        assert_eq!(mix(CELADON, INK, 1.0), INK);
    }

    #[test]
    fn rgba_normalises_bytes() {
        assert_eq!(rgba([255, 0, 51], 0.5), [1.0, 0.0, 0.2, 0.5]);
    }
}
