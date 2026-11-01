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

/// The colour scheme the menus and gameplay UI are drawn in, picked at
/// Setup -> Interface -> PALETTE (`ui_palette`). Classic is the original blue
/// slate with neon accents; Celadon is the five-tone palette above.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scheme {
    Classic,
    Celadon,
}

impl Scheme {
    pub(crate) const ALL: [Scheme; 2] = [Scheme::Classic, Scheme::Celadon];

    pub(crate) const fn index(self) -> i32 {
        match self {
            Scheme::Classic => 0,
            Scheme::Celadon => 1,
        }
    }

    /// Unknown values fall back to Classic.
    pub(crate) const fn from_index(index: i32) -> Self {
        match index {
            1 => Scheme::Celadon,
            _ => Scheme::Classic,
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Scheme::Classic => "Classic",
            Scheme::Celadon => "Celadon",
        }
    }

    pub(crate) const fn description(self) -> &'static str {
        match self {
            Scheme::Classic => "Blue slate surfaces with a sky-blue accent: the original menu look.",
            Scheme::Celadon => "Warm Stone Brown surfaces with Celadon green and Smoky Rose.",
        }
    }
}

static ACTIVE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub(crate) fn active() -> Scheme {
    Scheme::from_index(i32::from(ACTIVE.load(std::sync::atomic::Ordering::Relaxed)))
}

pub(crate) fn set_active(scheme: Scheme) {
    ACTIVE.store(scheme.index() as u8, std::sync::atomic::Ordering::Relaxed);
}

/// Gameplay-UI shades. The scoreboard, kill feed, console and vote menu draw
/// with these instead of ad-hoc literals. Each is a function of the active
/// scheme, named in capitals because it stands in for a constant.
pub(crate) mod hud {
    use super::*;

    /// Original slate tones, shared with the Classic egui menu theme.
    const SLATE_INK: [u8; 3] = [0x0B, 0x0F, 0x15];
    const SLATE_LINE: [u8; 3] = [0x3C, 0x49, 0x5A];
    const SLATE_TEXT: [u8; 3] = [0xEB, 0xF0, 0xF6];
    const SLATE_DIM: [u8; 3] = [0xAD, 0xBA, 0xC9];
    const SLATE_FAINT: [u8; 3] = [0x8B, 0x99, 0xAA];
    const SKY: [u8; 3] = [0x59, 0xA9, 0xFF];
    const CORAL: [u8; 3] = [0xFF, 0x74, 0x66];

    macro_rules! shades {
        ($($(#[$doc:meta])* $name:ident => $classic:expr, $celadon:expr;)*) => {
            $(
                $(#[$doc])*
                #[allow(non_snake_case)]
                pub(crate) fn $name() -> [f32; 4] {
                    match active() {
                        Scheme::Classic => $classic,
                        Scheme::Celadon => $celadon,
                    }
                }
            )*
        };
    }

    shades! {
        TEXT => rgba(SLATE_TEXT, 1.0), rgba(WHITE_SMOKE, 1.0);
        TEXT_DIM => rgba(SLATE_DIM, 1.0), rgba(SILVER, 1.0);
        TEXT_FAINT => rgba(SLATE_FAINT, 1.0), rgba(mix(SILVER, STONE_BROWN, 0.45), 1.0);
        ACCENT => rgba(SKY, 1.0), rgba(CELADON, 1.0);
        ROSE => rgba(CORAL, 1.0), rgba(SMOKY_ROSE, 1.0);
        ROSE_LIGHT => rgba(mix(CORAL, SLATE_TEXT, 0.35), 1.0), rgba(mix(SMOKY_ROSE, WHITE_SMOKE, 0.35), 1.0);
        RULE => rgba(SLATE_LINE, 1.0), rgba(STONE_BROWN, 1.0);

        /// Header/footer slab: opaque enough to hold text over any scene.
        SLAB_TOP => rgba(mix(SLATE_INK, SLATE_LINE, 0.30), 0.94), rgba(mix(INK, STONE_BROWN, 0.30), 0.94);
        SLAB_BOTTOM => rgba(SLATE_INK, 0.94), rgba(INK, 0.94);
        /// Translucent strip behind a single HUD line (kill feed, menus).
        STRIP => rgba(SLATE_INK, 0.62), rgba(INK, 0.62);
        SHADOW => [0.0, 0.0, 0.0, 0.32], [0.0, 0.0, 0.0, 0.32];

        /// Team colours, kept recognisable as red/blue; Celadon pulls them
        /// toward its warm, muted register.
        TEAM_RED => rgba([0xFF, 0x56, 0x4C], 1.0), rgba(mix(SMOKY_ROSE, [0xE0, 0x4A, 0x3E], 0.6), 1.0);
        TEAM_BLUE => rgba([0x4C, 0x8C, 0xFF], 1.0), rgba(mix([0x5A, 0x8C, 0xD8], WHITE_SMOKE, 0.12), 1.0);
        GOLD => rgba([0xFF, 0xB8, 0x3C], 1.0), rgba([0xE6, 0xC2, 0x6E], 1.0);

        /// Console: Classic keeps the original rain-slick night (indigo body,
        /// cyan and magenta neon, tungsten amber); Celadon follows the palette.
        CONSOLE_CYAN => [0.28, 0.86, 0.95, 1.0], rgba(CELADON, 1.0);
        CONSOLE_MAGENTA => [1.0, 0.30, 0.56, 1.0], rgba(mix(SMOKY_ROSE, WHITE_SMOKE, 0.35), 1.0);
        CONSOLE_AMBER => [1.0, 0.72, 0.28, 1.0], rgba([0xE6, 0xC2, 0x6E], 1.0);
        CONSOLE_TEXT => [0.80, 0.86, 0.93, 1.0], rgba(mix(WHITE_SMOKE, SILVER, 0.35), 1.0);
        CONSOLE_BRIGHT => [0.95, 0.97, 1.0, 1.0], rgba(WHITE_SMOKE, 1.0);
        CONSOLE_DIM => [0.50, 0.60, 0.73, 1.0], rgba(SILVER, 1.0);
        CONSOLE_FAINT => [0.32, 0.40, 0.52, 1.0], rgba(mix(SILVER, STONE_BROWN, 0.45), 1.0);
        /// Console surfaces: panel body (top and bottom), chips, search field,
        /// input bar and the two pop-up cards.
        CONSOLE_BODY_TOP => [0.012, 0.016, 0.036, 0.975], [0.062, 0.055, 0.047, 0.975];
        CONSOLE_BODY_BOTTOM => [0.024, 0.030, 0.066, 0.965], [0.090, 0.081, 0.069, 0.965];
        CONSOLE_CHIP => [0.04, 0.07, 0.13, 0.92], [0.11, 0.10, 0.085, 0.92];
        CONSOLE_FIELD => [0.03, 0.05, 0.10, 0.98], [0.085, 0.077, 0.065, 0.98];
        CONSOLE_BAR => [0.045, 0.065, 0.125, 0.94], [0.12, 0.108, 0.092, 0.94];
        CONSOLE_CARD => [0.020, 0.028, 0.056, 0.99], [0.075, 0.068, 0.058, 0.99];
        CONSOLE_SUGGEST_CARD => [0.020, 0.028, 0.056, 0.985], [0.075, 0.068, 0.058, 0.985];
        /// Gameplay HUD panels and their two text tones.
        HUD_INK_TOP => rgba(mix(SLATE_INK, SLATE_LINE, 0.22), 0.84), rgba(mix(INK, STONE_BROWN, 0.22), 0.84);
        HUD_INK_BOTTOM => rgba(SLATE_INK, 0.80), rgba(INK, 0.80);
        HUD_LABEL => rgba(SLATE_DIM, 1.0), rgba(SILVER, 1.0);
        HUD_VALUE => rgba(SLATE_TEXT, 1.0), rgba(WHITE_SMOKE, 1.0);
        /// Kill feed backing strip behind the highlighted lines.
        PARCHMENT => rgba(mix(SLATE_FAINT, SLATE_LINE, 0.55), 1.0), rgba(mix(SILVER, STONE_BROWN, 0.55), 1.0);
    }

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
