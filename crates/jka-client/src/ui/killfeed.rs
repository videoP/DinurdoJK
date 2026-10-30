//! Kill feed: the top-right obituary list, in five house styles.
//!
//! Entries come from EV_OBITUARY (see `App::note_obituary`), so live play,
//! demos and follow views all feed it. Each line is laid out with Taffy as
//! `[attacker][weapon][victim]` (or a sentence, for Chivalry) and the column
//! is right-aligned, so lines of different lengths share one right edge.
use crate::palette::hud as pal;
use crate::ui::flex::{Bounds, Container, Flex};
use crate::ui::hud::movement::{ICON_TEXTURE_SOURCE, ICON_WEAPON_BASE};
use crate::ui::text::UiText;
use crate::ui::{
    rect, rect_gradient, rect_outline, textured_rect_with_source, ProportionalFont, UiSnapshot,
    UiVertex,
};
use taffy::prelude::AlignItems;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KillfeedStyle {
    /// Dark strip per line, weapon icon between the names.
    #[default]
    CallOfDuty,
    /// No backing at all: shadowed text and icon, tight rows.
    Quake,
    /// Solid dark plates; your own lines get a rose outline.
    CounterStrike,
    /// Gradient that fades out leftward, gold for your own lines.
    Mordhau,
    /// A written line in the past tense: "Kyle slew Tavion".
    Chivalry,
}

impl KillfeedStyle {
    pub const ALL: [Self; 5] = [Self::CallOfDuty, Self::Quake, Self::CounterStrike, Self::Mordhau, Self::Chivalry];

    pub fn label(self) -> &'static str {
        match self {
            Self::CallOfDuty => "Call of Duty",
            Self::Quake => "Quake",
            Self::CounterStrike => "Counter-Strike",
            Self::Mordhau => "Mordhau",
            Self::Chivalry => "Chivalry",
        }
    }

    pub fn index(self) -> i32 {
        Self::ALL.iter().position(|&style| style == self).unwrap_or(0) as i32
    }

    pub fn from_index(index: i32) -> Self {
        usize::try_from(index).ok().and_then(|i| Self::ALL.get(i).copied()).unwrap_or_default()
    }
}

/// `cg_killfeed*` settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KillfeedSettings {
    pub enabled: bool,
    pub style: KillfeedStyle,
    /// Mark lines where you are the killer or the victim.
    pub highlight_own: bool,
    /// Seconds a line stays up: your kills, your deaths, everyone else.
    pub own_kill_time: f32,
    pub own_death_time: f32,
    pub other_time: f32,
    /// Peak opacity of each kind of line, 0..1.
    pub own_kill_alpha: f32,
    pub own_death_alpha: f32,
    pub other_alpha: f32,
    pub max_lines: u8,
}

impl Default for KillfeedSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            style: KillfeedStyle::CallOfDuty,
            highlight_own: true,
            own_kill_time: 8.0,
            own_death_time: 8.0,
            other_time: 5.0,
            own_kill_alpha: 1.0,
            own_death_alpha: 1.0,
            other_alpha: 0.85,
            max_lines: 6,
        }
    }
}

impl KillfeedSettings {
    /// Lifetime and peak opacity for a line.
    pub fn timing(&self, own_kill: bool, own_death: bool) -> (f32, f32) {
        if own_kill {
            (self.own_kill_time, self.own_kill_alpha)
        } else if own_death {
            (self.own_death_time, self.own_death_alpha)
        } else {
            (self.other_time, self.other_alpha)
        }
    }

    /// Opacity of a line `age` seconds old: a 120 ms fade in, the peak, then
    /// a 500 ms fade out at the end of its lifetime. `None` once expired.
    pub fn alpha_at(&self, age: f32, own_kill: bool, own_death: bool) -> Option<f32> {
        let (life, peak) = self.timing(own_kill, own_death);
        if age >= life || peak <= 0.0 {
            return None;
        }
        let fade_in = (age / 0.12).clamp(0.0, 1.0);
        let fade_out = ((life - age) / 0.5).clamp(0.0, 1.0);
        Some(peak.clamp(0.0, 1.0) * fade_in.min(fade_out))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct UiKillfeedEntry {
    /// `None` for a world/self kill: only the victim is shown.
    pub attacker: Option<String>,
    pub attacker_team: i32,
    pub target: String,
    pub target_team: i32,
    /// Means of death (MOD_*), for the weapon icon and Chivalry's verb.
    pub mod_: i32,
    pub own_kill: bool,
    pub own_death: bool,
    pub alpha: f32,
    /// 0 just arrived .. 1 settled; drives the slide-in.
    pub enter: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UiKillfeed {
    pub style: KillfeedStyle,
    pub highlight_own: bool,
    /// Newest first.
    pub entries: Vec<UiKillfeedEntry>,
}

/// JKA `meansOfDeath_t` to `weapon_t`, for the HUD weapon icon.
pub fn weapon_for_mod(mod_: i32) -> Option<usize> {
    Some(match mod_ {
        1 => 1,
        2 => 2,
        3 => 3,
        4 | 5 => 4,
        6 | 7 => 5,
        8..=10 => 6,
        11 => 7,
        12..=14 => 8,
        15 | 16 => 9,
        17 | 18 => 10,
        19..=22 => 11,
        23 | 24 => 12,
        25 | 26 => 13,
        27 => 14,
        29 | 30 => 15,
        _ => return None,
    })
}

/// Short tag for deaths that have no weapon art.
fn mod_tag(mod_: i32) -> &'static str {
    match mod_ {
        31 => "LIGHTNING",
        32 => "SENTRY",
        33 => "DROWNED",
        34 | 35 => "MELTED",
        36 => "CRUSHED",
        37 => "TELEFRAG",
        38 => "FELL",
        39 => "SUICIDE",
        40 => "LASER",
        _ => "KILLED",
    }
}

/// Chivalry's sentence verb.
fn mod_verb(mod_: i32) -> &'static str {
    match mod_ {
        3 => "cut down",
        1 | 2 => "beat down",
        4..=7 | 11..=14 => "shot",
        8..=10 => "picked off",
        15 | 16 => "shocked",
        17..=27 | 29 | 30 => "blew apart",
        31 => "struck down",
        37 => "crushed",
        _ => "slew",
    }
}

fn team_color(team: i32) -> [f32; 4] {
    match team {
        1 => pal::TEAM_RED,
        2 => pal::TEAM_BLUE,
        _ => pal::TEXT,
    }
}

pub(in crate::ui) fn build_killfeed(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    let Some(feed) = ui.killfeed.as_ref().filter(|feed| !feed.entries.is_empty()) else { return };
    let s = (h as f32 / 1080.0).clamp(0.6, 2.0);
    let style = feed.style;
    // Line heights in pixels at 1080p; the clean UI font when loaded.
    let px = match style {
        KillfeedStyle::Quake => 17.0,
        KillfeedStyle::Chivalry => 18.0,
        _ => 19.0,
    } * s;
    let txt = UiText::new(ui, small_font, w, h);
    let text_w = |value: &str| txt.width(value, px);
    let cap = txt.cap(px);
    let draw = |out: &mut Vec<UiVertex>, value: &str, x: f32, bounds: Bounds, color: [f32; 4]| {
        txt.draw(out, value, x, txt.baseline_in(bounds, px), px, color);
    };
    let row_h = match style {
        KillfeedStyle::Quake => 22.0,
        KillfeedStyle::CounterStrike => 30.0,
        _ => 27.0,
    } * s;
    let icon = row_h * 0.78;
    let pad_x = if style == KillfeedStyle::Quake { 2.0 * s } else { 10.0 * s };

    // One row per entry: [attacker][icon or tag][victim], or a sentence.
    let mut flex = Flex::new();
    let mut rows = Vec::new();
    let mut parts = Vec::new();
    for entry in &feed.entries {
        let mut children = Vec::new();
        let mut kinds = Vec::new();
        if style == KillfeedStyle::Chivalry {
            let line = match &entry.attacker {
                Some(attacker) => format!("{attacker}^7 {} {}", mod_verb(entry.mod_), entry.target),
                None => format!("{}^7 perished", entry.target),
            };
            children.push(flex.leaf(text_w(&line), cap));
            kinds.push(Part::Sentence(line));
        } else {
            if let Some(attacker) = &entry.attacker {
                children.push(flex.leaf(text_w(attacker), cap));
                kinds.push(Part::Attacker);
            }
            if weapon_for_mod(entry.mod_).is_some() {
                children.push(flex.leaf(icon, icon));
                kinds.push(Part::Icon);
            } else {
                let tag = mod_tag(entry.mod_);
                children.push(flex.leaf(text_w(tag), cap));
                kinds.push(Part::Tag(tag));
            }
            children.push(flex.leaf(text_w(&entry.target), cap));
            kinds.push(Part::Target);
        }
        let row = flex.node(Container::row().height(row_h).pad(pad_x, 0.0).gap(8.0 * s), &children);
        rows.push(row);
        parts.push(children.into_iter().zip(kinds).collect::<Vec<_>>());
    }
    // A fixed-width column anchored to the top-right of the stock rect; every
    // row hugs its right edge.
    let [column_x, column_y, column_w, _] =
        column_bounds(w, h, corner_rows(ui), ui.enemy_info.is_some());
    let gap = if style == KillfeedStyle::Quake { 1.0 } else { 3.0 } * s;
    let column = flex.absolute(
        Container::column().width(column_w).gap(gap).align(AlignItems::FLEX_END),
        column_x,
        column_y,
        &rows,
    );
    let screen = flex.node(Container::column().width(w as f32).height(h as f32), &[column]);
    flex.compute(screen, w as f32, h as f32);

    for ((entry, &row), parts) in feed.entries.iter().zip(&rows).zip(&parts) {
        let alpha = entry.alpha.clamp(0.0, 1.0);
        if alpha <= 0.0 {
            continue;
        }
        let slide = (1.0 - entry.enter.clamp(0.0, 1.0)).powi(2) * 36.0 * s;
        let mut b = flex.bounds(row);
        b[0] += slide;
        let own = feed.highlight_own && (entry.own_kill || entry.own_death);
        let own_tone = if entry.own_death && !entry.own_kill { pal::ROSE_LIGHT } else { pal::ACCENT };
        let hairline = (1.0 * s).max(1.0);
        match style {
            KillfeedStyle::CallOfDuty => {
                rect(out, b[0], b[1], b[2], b[3], pal::fade(pal::STRIP, alpha * if own { 1.25 } else { 1.0 }), w, h);
                if own {
                    rect(out, b[0], b[1], b[2], b[3], pal::fade(own_tone, 0.10 * alpha), w, h);
                    rect(out, b[0], b[1], (3.0 * s).max(2.0), b[3], pal::fade(own_tone, alpha), w, h);
                }
            }
            KillfeedStyle::Quake => {
                if own {
                    rect(out, b[0], b[1] + b[3] - hairline * 2.0, b[2], hairline * 2.0, pal::fade(own_tone, 0.8 * alpha), w, h);
                }
            }
            KillfeedStyle::CounterStrike => {
                rect(out, b[0], b[1], b[2], b[3], [0.0, 0.0, 0.0, 0.58 * alpha], w, h);
                if own {
                    rect_outline(out, b[0], b[1], b[2], b[3], (2.0 * s).max(1.5), pal::fade(pal::ROSE, alpha), w, h);
                }
            }
            KillfeedStyle::Mordhau => {
                let tint = if own { pal::GOLD } else { [0.0, 0.0, 0.0, 1.0] };
                let strength = if own { 0.30 } else { 0.55 };
                let left = [tint[0], tint[1], tint[2], 0.0];
                let right = [tint[0], tint[1], tint[2], strength * alpha];
                rect_gradient(out, b[0], b[1], b[2], b[3], left, right, left, right, w, h);
                rect(out, b[0] + b[2] * 0.3, b[1] + b[3] - hairline, b[2] * 0.7, hairline, pal::fade(if own { pal::GOLD } else { pal::RULE }, 0.7 * alpha), w, h);
            }
            KillfeedStyle::Chivalry => {
                let parchment = pal::fade(crate::palette::rgba(crate::palette::mix(crate::palette::SILVER, crate::palette::STONE_BROWN, 0.55), 1.0), 0.55 * alpha);
                rect(out, b[0], b[1], b[2], b[3], parchment, w, h);
                rect(out, b[0], b[1] + b[3] - hairline, b[2], hairline, pal::fade(if own { pal::GOLD } else { pal::TEXT_FAINT }, alpha), w, h);
            }
        }
        for &(node, ref part) in parts {
            let mut nb = flex.bounds(node);
            nb[0] += slide;
            match part {
                Part::Attacker => {
                    let color = if own && entry.own_kill && style != KillfeedStyle::CounterStrike { pal::ACCENT } else { team_color(entry.attacker_team) };
                    draw(out, entry.attacker.as_deref().unwrap_or_default(), nb[0], nb, pal::fade(color, alpha));
                }
                Part::Target => {
                    let color = if own && entry.own_death && style != KillfeedStyle::CounterStrike { pal::ROSE_LIGHT } else { team_color(entry.target_team) };
                    draw(out, &entry.target, nb[0], nb, pal::fade(color, alpha));
                }
                Part::Tag(tag) => draw(out, tag, nb[0], nb, pal::fade(pal::TEXT_DIM, alpha)),
                Part::Sentence(line) => draw(out, line, nb[0], nb, pal::fade(pal::TEXT, alpha)),
                Part::Icon => {
                    let Some(weapon) = weapon_for_mod(entry.mod_) else { continue };
                    let (uv0, uv1) = ui.icon_uv(ICON_WEAPON_BASE + weapon);
                    // Quake draws the icon bare; every other style tints it
                    // slightly toward the text colour so it sits with the names.
                    let tint = if style == KillfeedStyle::Quake { [1.0, 1.0, 1.0, alpha] } else { pal::fade(pal::TEXT, alpha) };
                    textured_rect_with_source(out, nb[0], nb[1], nb[2], nb[3], uv0, uv1, tint, ICON_TEXTURE_SOURCE, w, h);
                }
            }
        }
    }
}

enum Part {
    Attacker,
    Icon,
    Tag(&'static str),
    Target,
    Sentence(String),
}

/// Text rows `CG_DrawUpperRight` stacks in the top-right corner: FPS, the
/// timer, and cg_drawScores in its corner modes (1/2; mode 3 is centred).
pub(in crate::ui) fn corner_rows(ui: &UiSnapshot) -> u8 {
    let team_scores = matches!(ui.mini_scores, Some(crate::ui::UiMiniScores::Team { mode, .. }) if mode != 3);
    u8::from(ui.video.draw_fps != 0) + u8::from(ui.game_timer.is_some()) + u8::from(team_scores)
}

/// Where the feed's column sits before any HUD Edit offset: hung from the
/// top-right corner, below the corner text rows, and left of the duel Enemy
/// Info panel (`CG_DrawUpperRight`, 135 virtual px wide) while it is up.
/// Shared with `hud_stock_rect` so the HUD Edit outline matches the drawing.
pub(in crate::ui) fn column_bounds(w: u32, h: u32, rows: u8, enemy_info: bool) -> [f32; 4] {
    let s = (h as f32 / 1080.0).clamp(0.6, 2.0);
    let width = 560.0 * s;
    let top = 16.0 * s + f32::from(rows) * 22.0 * h as f32 / 480.0;
    let mut right = w as f32 - 16.0 * s;
    if enemy_info {
        right -= 135.0 * w as f32 / 640.0 + 12.0 * s;
    }
    [right - width, top, width, 6.0 * 30.0 * s]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_kills_outlast_other_lines_by_default() {
        let settings = KillfeedSettings::default();
        assert!(settings.alpha_at(6.0, true, false).is_some());
        assert!(settings.alpha_at(6.0, false, false).is_none());
    }

    #[test]
    fn alpha_fades_in_and_out_to_the_configured_peak() {
        let settings = KillfeedSettings { other_alpha: 0.5, ..KillfeedSettings::default() };
        assert_eq!(settings.alpha_at(0.0, false, false), Some(0.0));
        assert_eq!(settings.alpha_at(1.0, false, false), Some(0.5));
        let late = settings.alpha_at(4.75, false, false).unwrap();
        assert!(late > 0.0 && late < 0.5);
    }

    #[test]
    fn weapon_mapping_covers_saber_and_explosives() {
        assert_eq!(weapon_for_mod(3), Some(3));
        assert_eq!(weapon_for_mod(20), Some(11));
        assert_eq!(weapon_for_mod(38), None);
    }

    #[test]
    fn style_index_round_trips() {
        for style in KillfeedStyle::ALL {
            assert_eq!(KillfeedStyle::from_index(style.index()), style);
        }
        assert_eq!(KillfeedStyle::from_index(99), KillfeedStyle::CallOfDuty);
    }
}
