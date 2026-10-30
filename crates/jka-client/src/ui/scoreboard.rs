//! Scoreboard.
//!
//! Split down the middle like a team shooter's board: red on the left, blue on
//! the right (free-for-all fills the left half, then the right). Both halves
//! read the same way: avatar, name, score, then the extra numbers (deaths,
//! head-to-head, server awards) and ping and time at the far right. The centre
//! divider carries the one scrollbar. Spectators sit in a strip
//! under both halves. The footer toggles a personal match breakdown, which
//! takes over the body at intermission.
//!
//! Everything is laid out with Taffy (`ui::flex`) in `board_layout`, which the
//! renderer and the app's pointer hit testing both call.
use crate::palette::hud as pal;
use crate::ui::flex::{contains, Bounds, Container, Flex};
use crate::ui::text::UiText;
use crate::ui::{
    glyphs, player_icon_atlas_cell, rect, rect_gradient, rect_outline,
    scoreboard_icon_atlas_offset, shared_player_icon_count, truncate_jka_text, ProportionalFont,
    ScoreAction, ScoreboardHit, ScoreboardMenuUi, UiMatchStats, UiScoreEntry, UiScoreboard,
    UiSnapshot, UiVertex,
};
use taffy::prelude::{AlignItems, JustifyContent};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    Avatar,
    Name,
    Score,
    Deaths,
    Captures,
    HeadToHead,
    Accuracy,
    Excellent,
    Baton,
    Defends,
    Assists,
    Ping,
    Time,
}

impl Slot {
    fn caption(self) -> &'static str {
        match self {
            Self::Name | Self::Avatar => "",
            Self::HeadToHead => "VS YOU",
            Self::Score => "SCORE",
            Self::Deaths => "DEATHS",
            Self::Captures => "CAPS",
            Self::Accuracy => "ACC",
            Self::Excellent => "EXC",
            Self::Baton => "BATON",
            Self::Defends => "DEF",
            Self::Assists => "AST",
            Self::Ping => "PING",
            Self::Time => "TIME",
        }
    }

    /// The server counter behind an optional column.
    fn counter(self, entry: &UiScoreEntry) -> Option<i32> {
        match self {
            Self::Accuracy => Some(entry.accuracy),
            Self::Excellent => Some(entry.excellent),
            Self::Baton => Some(entry.gauntlet),
            Self::Defends => Some(entry.defends),
            Self::Assists => Some(entry.assists),
            _ => None,
        }
    }
}

pub(crate) struct Half {
    pub side: Side,
    /// Team shown in this half (0 for free-for-all).
    pub team: i32,
    pub panel: Bounds,
    pub header: Bounds,
    /// Column slots, `(kind, x, width)` in absolute pixels.
    pub slots: Vec<(Slot, f32, f32)>,
    /// Visible rows: bounds and index into `UiScoreboard::entries`.
    pub rows: Vec<(Bounds, usize)>,
    /// Every entry in this half, in order.
    pub members: Vec<usize>,
}

pub(crate) struct BoardLayout {
    pub s: f32,
    pub panel: Bounds,
    pub server: Bounds,
    pub subtitle: Bounds,
    pub score_block: Bounds,
    pub body: Bounds,
    pub halves: [Half; 2],
    pub divider: Bounds,
    pub spectator_band: Option<Bounds>,
    pub spectators: Vec<(Bounds, usize)>,
    pub players: usize,
    pub max_scroll: usize,
    pub scrollbar: Option<(Bounds, Bounds)>,
    pub stats_open: bool,
    pub footer: Bounds,
    pub footer_left: Bounds,
    pub footer_right: Bounds,
    pub stats_toggle: Bounds,
    pub vote_yes: Option<Bounds>,
    pub vote_no: Option<Bounds>,
}

pub(crate) struct MenuLayout {
    pub panel: Bounds,
    pub title: Bounds,
    pub buttons: Vec<Bounds>,
}

/// Line heights in pixels at 1080p; multiplied by `scale`.
mod size {
    pub const SERVER: f32 = 30.0;
    pub const SUBTITLE: f32 = 15.0;
    pub const CAPTION: f32 = 13.0;
    pub const HALF_TITLE: f32 = 16.0;
    pub const ROW: f32 = 20.0;
    pub const SMALL: f32 = 13.0;
    pub const META: f32 = 15.0;
    pub const TEAM_SCORE: f32 = 30.0;
    pub const STAT_VALUE: f32 = 30.0;
}

fn scale(h: u32) -> f32 {
    (h as f32 / 1080.0).clamp(0.72, 2.0)
}

/// Which entries go in the left and right halves, and the spectators.
pub(crate) fn split(board: &UiScoreboard) -> ([Vec<usize>; 2], Vec<usize>, [i32; 2]) {
    let spectators: Vec<usize> = (0..board.entries.len()).filter(|&i| board.entries[i].team == 3).collect();
    if board.team_game {
        let mut red: Vec<usize> = (0..board.entries.len()).filter(|&i| board.entries[i].team == 1).collect();
        let mut blue: Vec<usize> = (0..board.entries.len()).filter(|&i| board.entries[i].team == 2).collect();
        // Rare teamless players in a team mode join the shorter side.
        for i in (0..board.entries.len()).filter(|&i| board.entries[i].team == 0) {
            if red.len() <= blue.len() { red.push(i) } else { blue.push(i) }
        }
        ([red, blue], spectators, [1, 2])
    } else {
        let players: Vec<usize> = (0..board.entries.len()).filter(|&i| board.entries[i].team != 3).collect();
        let cut = players.len().div_ceil(2);
        ([players[..cut].to_vec(), players[cut..].to_vec()], spectators, [0, 0])
    }
}

/// Left to right, the same in both halves: who, score, then everything the
/// stock board does not show, then ping and time at the far right.
fn slot_order(board: &UiScoreboard) -> Vec<Slot> {
    let mut slots = Vec::new();
    if board.chrome.avatars {
        slots.push(Slot::Avatar);
    }
    slots.extend([Slot::Name, Slot::Score]);
    if board.ctf {
        slots.push(Slot::Captures);
    } else if board.show_deaths {
        slots.push(Slot::Deaths);
    }
    if board.chrome.head_to_head {
        slots.push(Slot::HeadToHead);
    }
    // Server counters most servers leave at 0 for everyone (saber-only play,
    // no CTF): a column only appears once someone has a real value.
    for slot in [Slot::Accuracy, Slot::Excellent, Slot::Baton, Slot::Defends, Slot::Assists] {
        if board.entries.iter().any(|entry| slot.counter(entry).unwrap_or(0) > 0) {
            slots.push(slot);
        }
    }
    slots.extend([Slot::Ping, Slot::Time]);
    slots
}

fn slot_width(slot: Slot, s: f32) -> f32 {
    match slot {
        Slot::Name => 0.0,
        Slot::Score | Slot::Deaths | Slot::Captures => 62.0 * s,
        Slot::Ping | Slot::Time | Slot::Baton => 54.0 * s,
        Slot::Accuracy | Slot::Excellent | Slot::Defends | Slot::Assists => 48.0 * s,
        Slot::HeadToHead => 76.0 * s,
        Slot::Avatar => 44.0 * s,
    }
}

/// The whole board for a `w` x `h` framebuffer.
pub(crate) fn board_layout(board: &UiScoreboard, w: u32, h: u32) -> BoardLayout {
    let s = scale(h);
    let (sw, sh) = (w as f32, h as f32);
    let width = (1480.0 * s).min((sw - 48.0 * s).max(360.0 * s));
    let x = ((sw - width) * 0.5).max(16.0 * s);
    let y = (sh * 0.06).max(24.0 * s);
    let pad = 24.0 * s;
    let header_pad_y = 16.0 * s;
    let server_h = 38.0 * s;
    let subtitle_h = 20.0 * s;
    let half_header_h = 38.0 * s;
    let row_h = 44.0 * s;
    let footer_h = 42.0 * s;
    let divider_w = 18.0 * s;
    let header_h = header_pad_y * 2.0 + server_h + 4.0 * s + subtitle_h;

    let ([left, right], spectators, teams) = split(board);
    let players = left.len() + right.len();
    let stats_open = board.chrome.stats_open && board.chrome.stats.is_some();
    let spectator_rows = if spectators.is_empty() || stats_open { 0 } else { spectators.len().div_ceil(6) };
    let spectator_h = if spectator_rows == 0 { 0.0 } else { 14.0 * s + spectator_rows as f32 * 30.0 * s };
    let available = (sh - y - 18.0 * s - header_h - footer_h - spectator_h).max(half_header_h + row_h);
    let visible = (((available - half_header_h) / row_h).floor() as usize).max(1);
    let longest = left.len().max(right.len());
    let max_scroll = if stats_open { 0 } else { longest.saturating_sub(visible) };
    let scroll = usize::from(board.chrome.scroll).min(max_scroll);
    let shown = longest.saturating_sub(scroll).min(visible).max(1);
    let body_h = if stats_open || max_scroll > 0 { available } else { half_header_h + row_h * shown as f32 };

    let mut flex = Flex::new();
    let server = flex.spring();
    let score_block = flex.leaf(380.0 * s, server_h);
    let top_row = flex.node(Container::row().height(server_h).gap(16.0 * s), &[server, score_block]);
    let subtitle = flex.band(subtitle_h);
    let header = flex.node(Container::column().pad(pad, header_pad_y).gap(4.0 * s), &[top_row, subtitle]);
    let half_nodes = [flex.spring(), flex.spring()];
    let divider = flex.node(Container::column().width(divider_w), &[]);
    let body = flex.node(
        Container::row().height(body_h).align(AlignItems::STRETCH).pad(10.0 * s, 0.0),
        &[half_nodes[0], divider, half_nodes[1]],
    );
    let spectator_band = (spectator_h > 0.0).then(|| flex.band(spectator_h));
    let footer_left = flex.spring();
    let footer_right = flex.spring();
    let stats_toggle = flex.leaf(132.0 * s, 26.0 * s);
    let mut footer_children = vec![footer_left];
    let vote_nodes = board.chrome.vote.as_ref().map(|_| (flex.leaf(96.0 * s, 26.0 * s), flex.leaf(96.0 * s, 26.0 * s)));
    if let Some((yes, no)) = vote_nodes {
        footer_children.extend([yes, no]);
    }
    footer_children.extend([footer_right, stats_toggle]);
    let footer = flex.node(
        Container::row().height(footer_h).pad(pad, 0.0).gap(10.0 * s).justify(JustifyContent::SPACE_BETWEEN),
        &footer_children,
    );
    let mut panel_children = vec![header, body];
    panel_children.extend(spectator_band);
    panel_children.push(footer);
    let panel = flex.absolute(Container::column().width(width), x, y, &panel_children);
    let screen = flex.node(Container::column().width(sw).height(sh), &[panel]);
    flex.compute(screen, sw, sh);

    // One column template, used by both halves.
    let order = slot_order(board);
    let make_half = |side: Side, node, members: &Vec<usize>, team: i32| -> Half {
        let panel = flex.bounds(node);
        let header = [panel[0], panel[1], panel[2], half_header_h];
        let mut columns = Flex::new();
        let kinds = &order;
        let nodes: Vec<_> = kinds
            .iter()
            .map(|&slot| if slot == Slot::Name { columns.spring() } else { columns.leaf(slot_width(slot, s), 1.0) })
            .collect();
        let row = columns.node(Container::row().width(panel[2]).pad(14.0 * s, 0.0).gap(4.0 * s), &nodes);
        columns.compute(row, panel[2], 1.0);
        let slots = kinds
            .iter()
            .zip(&nodes)
            .map(|(&slot, &node)| {
                let b = columns.bounds(node);
                (slot, panel[0] + b[0], b[2])
            })
            .collect();
        let rows = members
            .iter()
            .skip(scroll)
            .take(shown)
            .enumerate()
            .map(|(i, &entry)| ([panel[0], panel[1] + half_header_h + i as f32 * row_h, panel[2], row_h], entry))
            .collect();
        Half { side, team, panel, header, slots, rows, members: members.clone() }
    };
    let halves = [
        make_half(Side::Left, half_nodes[0], &left, teams[0]),
        make_half(Side::Right, half_nodes[1], &right, teams[1]),
    ];

    let divider_b = flex.bounds(divider);
    let scrollbar = (max_scroll > 0).then(|| {
        let track = [
            divider_b[0] + divider_b[2] * 0.5 - 2.0 * s,
            divider_b[1] + half_header_h + 6.0 * s,
            4.0 * s,
            (divider_b[3] - half_header_h - 12.0 * s).max(8.0),
        ];
        let thumb_h = (track[3] * visible as f32 / longest.max(1) as f32).max(18.0 * s).min(track[3]);
        let thumb_y = track[1] + (track[3] - thumb_h) * scroll as f32 / max_scroll.max(1) as f32;
        (track, [track[0], thumb_y, track[2], thumb_h])
    });

    // Spectators: name chips flowing across the strip, six to a line.
    let band = spectator_band.map(|node| flex.bounds(node));
    let spectator_chips = band
        .map(|band| {
            let chip_w = (band[2] - 2.0 * pad - 110.0 * s) / 6.0;
            spectators
                .iter()
                .enumerate()
                .map(|(i, &entry)| {
                    let (col, line) = ((i % 6) as f32, (i / 6) as f32);
                    ([band[0] + pad + 110.0 * s + col * chip_w, band[1] + 8.0 * s + line * 30.0 * s, chip_w - 6.0 * s, 26.0 * s], entry)
                })
                .collect()
        })
        .unwrap_or_default();

    BoardLayout {
        s,
        panel: flex.bounds(panel),
        server: flex.bounds(server),
        subtitle: flex.bounds(subtitle),
        score_block: flex.bounds(score_block),
        body: flex.bounds(body),
        halves,
        divider: divider_b,
        spectator_band: band,
        spectators: spectator_chips,
        players,
        max_scroll,
        scrollbar,
        stats_open,
        footer: flex.bounds(footer),
        footer_left: flex.bounds(footer_left),
        footer_right: flex.bounds(footer_right),
        stats_toggle: flex.bounds(stats_toggle),
        vote_yes: vote_nodes.map(|(yes, _)| flex.bounds(yes)),
        vote_no: vote_nodes.map(|(_, no)| flex.bounds(no)),
    }
}

/// The player popup: a title band and one button per action, opened at the
/// click and pushed back on screen when it would overflow an edge.
pub(crate) fn menu_layout(menu: &ScoreboardMenuUi, w: u32, h: u32) -> MenuLayout {
    let s = scale(h);
    let (sw, sh) = (w as f32, h as f32);
    let width = 240.0 * s;
    let button_h = 34.0 * s;
    let title_h = 36.0 * s;
    let height = 8.0 * s * 2.0 + title_h + menu.actions.len() as f32 * (button_h + 3.0 * s);
    let x = (menu.anchor[0] as f32 + 6.0 * s).min(sw - width - 8.0 * s).max(8.0 * s);
    let y = (menu.anchor[1] as f32 + 6.0 * s).min(sh - height - 8.0 * s).max(8.0 * s);
    let mut flex = Flex::new();
    let title = flex.band(title_h);
    let buttons: Vec<_> = menu.actions.iter().map(|_| flex.band(button_h)).collect();
    let mut children = vec![title];
    children.extend(&buttons);
    let panel = flex.absolute(Container::column().width(width).pad(8.0 * s, 8.0 * s).gap(3.0 * s), x, y, &children);
    let screen = flex.node(Container::column().width(sw).height(sh), &[panel]);
    flex.compute(screen, sw, sh);
    MenuLayout {
        panel: flex.bounds(panel),
        title: flex.bounds(title),
        buttons: buttons.iter().map(|&node| flex.bounds(node)).collect(),
    }
}

/// What is under `pointer` (physical pixels). The open menu wins over rows.
pub(crate) fn hit_test(board: &UiScoreboard, w: u32, h: u32, pointer: [f32; 2]) -> Option<ScoreboardHit> {
    if let Some(menu) = board.chrome.cursor.as_ref().and_then(|cursor| cursor.menu.as_ref()) {
        let layout = menu_layout(menu, w, h);
        if let Some(index) = layout.buttons.iter().position(|&b| contains(b, pointer)) {
            return Some(ScoreboardHit::Action(index));
        }
        if contains(layout.panel, pointer) {
            return None;
        }
    }
    let layout = board_layout(board, w, h);
    if layout.vote_yes.is_some_and(|b| contains(b, pointer)) {
        return Some(ScoreboardHit::VoteYes);
    }
    if layout.vote_no.is_some_and(|b| contains(b, pointer)) {
        return Some(ScoreboardHit::VoteNo);
    }
    if board.chrome.stats.is_some() && contains(layout.stats_toggle, pointer) {
        return Some(ScoreboardHit::StatsToggle);
    }
    if layout.stats_open {
        return None;
    }
    let row_hit = layout
        .halves
        .iter()
        .flat_map(|half| half.rows.iter())
        .chain(layout.spectators.iter())
        .find(|(bounds, _)| contains(*bounds, pointer));
    row_hit.and_then(|&(_, index)| board.entries.get(index)).map(|entry| ScoreboardHit::Row(entry.client))
}

/// Largest useful scroll offset for this board on a `w` x `h` framebuffer.
pub(crate) fn max_scroll(board: &UiScoreboard, w: u32, h: u32) -> usize {
    board_layout(board, w, h).max_scroll
}

fn team_tone(team: i32) -> [f32; 4] {
    match team {
        1 => pal::TEAM_RED,
        2 => pal::TEAM_BLUE,
        _ => pal::ACCENT,
    }
}

pub(in crate::ui) fn build_scoreboard(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    let Some(board) = &ui.scoreboard else { return };
    let layout = board_layout(board, w, h);
    let s = layout.s;
    let hairline = (1.0 * s).max(1.0);
    let cursor = board.chrome.cursor.as_ref();
    let hovered = cursor.and_then(|cursor| cursor.hovered);
    let menu_client = cursor.and_then(|cursor| cursor.menu.as_ref()).map(|menu| menu.client);
    let txt = UiText::new(ui, small_font, w, h);
    let px = |size: f32| size * s;

    // ---- header slab -------------------------------------------------------
    let [px0, py, pw, _] = layout.panel;
    let slab_bottom = layout.body[1];
    rect(out, px0 - 3.0 * s, py + 4.0 * s, pw + 6.0 * s, slab_bottom - py, pal::SHADOW, w, h);
    rect_gradient(out, px0, py, pw, slab_bottom - py, pal::SLAB_TOP, pal::SLAB_TOP, pal::SLAB_BOTTOM, pal::SLAB_BOTTOM, w, h);
    rect(out, px0, py, pw, (2.0 * s).max(1.0), pal::ACCENT, w, h);
    let server_name = if board.chrome.server_name.is_empty() { "SCOREBOARD" } else { board.chrome.server_name.as_str() };
    let mut chars = 48usize;
    let mut server = truncate_jka_text(server_name, chars);
    while chars > 8 && txt.width(&server, px(size::SERVER)) > layout.server[2] {
        chars -= 2;
        server = truncate_jka_text(server_name, chars);
    }
    txt.draw(out, &server, layout.server[0], txt.baseline_in(layout.server, px(size::SERVER)), px(size::SERVER), pal::TEXT);
    let subtitle = if board.siege && board.siege_win_team != 0 {
        format!("{}  ·  {} TEAM WINS", board.chrome.subtitle, if board.siege_win_team == 1 { "RED" } else { "BLUE" })
    } else {
        board.chrome.subtitle.clone()
    };
    txt.draw(out, &subtitle, layout.subtitle[0], txt.baseline_in(layout.subtitle, px(size::SUBTITLE)), px(size::SUBTITLE), pal::TEXT_DIM);
    let block = layout.score_block;
    let score_right = block[0] + block[2];
    if board.entries.is_empty() {
        txt.draw_right(out, "AWAITING SCORES", score_right, txt.baseline_in(block, px(size::META)), px(size::META), pal::TEXT_FAINT);
    } else if board.team_game {
        let size = px(size::TEAM_SCORE);
        let baseline = txt.baseline_in(block, size);
        let sabers = block[3] * 0.95;
        let blue = format!("{}  BLUE", board.team_scores[1]);
        let red = format!("RED  {}", board.team_scores[0]);
        let blue_x = score_right - txt.width(&blue, size);
        let sabers_x = blue_x - 10.0 * s - sabers;
        let red_x = sabers_x - 10.0 * s - txt.width(&red, size);
        txt.draw(out, &red, red_x, baseline, size, pal::TEAM_RED);
        // Blades in full saber colours; the muted team tones read grey at this size.
        glyphs::crossed_sabers(out, [sabers_x, block[1] + (block[3] - sabers) * 0.5, sabers, sabers], [1.0, 0.22, 0.16, 1.0], [0.24, 0.55, 1.0, 1.0], w, h);
        txt.draw(out, &blue, blue_x, baseline, size, pal::TEAM_BLUE);
    } else {
        let label = format!("{} PLAYER{}", layout.players, if layout.players == 1 { "" } else { "S" });
        txt.draw_right(out, &label, score_right, txt.baseline_in(block, px(size::META)), px(size::META), pal::TEXT_DIM);
    }

    // ---- body ---------------------------------------------------------------
    if layout.stats_open {
        if let Some(stats) = &board.chrome.stats {
            draw_stats(out, stats, layout.body, s, &txt, w, h);
        }
    } else {
        for half in &layout.halves {
            draw_half(out, board, ui, half, &layout, hovered, menu_client, &txt, w, h);
        }
        // Divider and the shared scrollbar.
        let d = layout.divider;
        rect(out, d[0] + d[2] * 0.5 - hairline * 0.5, d[1], hairline, d[3], pal::fade(pal::RULE, 0.6), w, h);
        if let Some((track, thumb)) = layout.scrollbar {
            rect(out, track[0], track[1], track[2], track[3], pal::fade(pal::RULE, 0.55), w, h);
            rect(out, thumb[0], thumb[1], thumb[2], thumb[3], pal::fade(pal::ACCENT, 0.9), w, h);
        }
        if let Some(band) = layout.spectator_band {
            rect(out, band[0], band[1], band[2], band[3], pal::fade(pal::SLAB_BOTTOM, 0.55), w, h);
            txt.draw(out, "SPECTATORS", band[0] + 24.0 * s, band[1] + 8.0 * s + txt.cap(px(size::CAPTION)) + 6.0 * s, px(size::CAPTION), pal::TEXT_FAINT);
            for &(chip, index) in &layout.spectators {
                let Some(entry) = board.entries.get(index) else { continue };
                let lit = hovered == Some(ScoreboardHit::Row(entry.client)) || menu_client == Some(entry.client);
                rect(out, chip[0], chip[1], chip[2], chip[3], pal::fade(if lit { pal::ACCENT } else { pal::TEXT }, if lit { 0.18 } else { 0.05 }), w, h);
                let size = px(size::SMALL);
                let name = truncate_jka_text(&entry.name, 18);
                txt.draw(out, &name, chip[0] + 8.0 * s, txt.baseline_in(chip, size), size, pal::fade(pal::TEXT, 0.8));
                if entry.muted {
                    let icon = 12.0 * s;
                    glyphs::muted(out, [chip[0] + chip[2] - icon * 1.3, chip[1] + (chip[3] - icon) * 0.5, icon * 1.15, icon], 1.0, w, h);
                }
            }
        }
    }

    // ---- footer slab ------------------------------------------------------
    let [fx, fy, fw, fh] = layout.footer;
    rect(out, fx - 3.0 * s, fy + 4.0 * s, fw + 6.0 * s, fh, pal::SHADOW, w, h);
    rect_gradient(out, fx, fy, fw, fh, pal::SLAB_TOP, pal::SLAB_TOP, pal::SLAB_BOTTOM, pal::SLAB_BOTTOM, w, h);
    rect(out, fx, fy, fw, hairline, pal::fade(pal::ACCENT, 0.55), w, h);
    let meta = px(size::META);
    let footer_baseline = txt.baseline_in(layout.footer, meta);
    let mut left_x = layout.footer_left[0];
    for (label, value) in [("JOINED", board.chrome.joined.as_deref()), ("PLAYTIME", board.chrome.playtime.as_deref())] {
        let Some(value) = value else { continue };
        txt.draw(out, label, left_x, footer_baseline, meta, pal::TEXT_FAINT);
        left_x += txt.width(label, meta) + 8.0 * s;
        txt.draw(out, value, left_x, footer_baseline, meta, pal::ACCENT);
        left_x += txt.width(value, meta) + 26.0 * s;
    }
    let hint = match cursor {
        Some(cursor) if cursor.menu.is_some() => "CLICK OUTSIDE TO CLOSE",
        Some(_) => "CLICK A PLAYER FOR OPTIONS   ·   M2 / ESC  RELEASE",
        None if layout.max_scroll > 0 => "WHEEL  SCROLL   ·   M2 / M3  MOUSE",
        None => "M2 / M3  MOUSE",
    };
    txt.draw_right(out, hint, layout.footer_right[0] + layout.footer_right[2], footer_baseline, meta, pal::TEXT_FAINT);
    if board.chrome.stats.is_some() {
        let b = layout.stats_toggle;
        let lit = hovered == Some(ScoreboardHit::StatsToggle);
        let label = if layout.stats_open { "PLAYERS" } else { "MY MATCH" };
        rect(out, b[0], b[1], b[2], b[3], pal::fade(pal::GOLD, if lit { 0.30 } else { 0.12 }), w, h);
        rect_outline(out, b[0], b[1], b[2], b[3], hairline, pal::fade(pal::GOLD, if lit { 1.0 } else { 0.55 }), w, h);
        txt.draw_centered(out, label, b, px(size::CAPTION), if lit { pal::TEXT } else { pal::GOLD });
    }
    if let (Some(yes), Some(no)) = (layout.vote_yes, layout.vote_no) {
        for (bounds, label, hit, tone) in [
            (yes, "VOTE YES", ScoreboardHit::VoteYes, pal::ACCENT),
            (no, "VOTE NO", ScoreboardHit::VoteNo, pal::ROSE_LIGHT),
        ] {
            let lit = hovered == Some(hit);
            rect(out, bounds[0], bounds[1], bounds[2], bounds[3], pal::fade(tone, if lit { 0.32 } else { 0.14 }), w, h);
            rect_outline(out, bounds[0], bounds[1], bounds[2], bounds[3], hairline, pal::fade(tone, if lit { 1.0 } else { 0.6 }), w, h);
            txt.draw_centered(out, label, bounds, px(size::CAPTION), if lit { pal::TEXT } else { tone });
        }
        if let Some(vote) = &board.chrome.vote {
            let x = layout.vote_yes.map_or(fx, |b| b[0]);
            txt.draw_right(out, &truncate_jka_text(vote, 60), x - 12.0 * s, footer_baseline, meta, pal::TEXT);
        }
    }

    // ---- player popup -----------------------------------------------------
    if let Some(menu) = cursor.and_then(|cursor| cursor.menu.as_ref()) {
        let popup = menu_layout(menu, w, h);
        let [mx, my, mw, mh] = popup.panel;
        rect(out, mx + 3.0 * s, my + 5.0 * s, mw, mh, [0.0, 0.0, 0.0, 0.40], w, h);
        rect_gradient(out, mx, my, mw, mh, pal::SLAB_TOP, pal::SLAB_TOP, pal::SLAB_BOTTOM, pal::SLAB_BOTTOM, w, h);
        rect_outline(out, mx, my, mw, mh, hairline, pal::RULE, w, h);
        rect(out, mx, my, mw, (2.0 * s).max(1.0), pal::ACCENT, w, h);
        let title_size = px(size::ROW) * 0.95;
        let name = truncate_jka_text(&menu.name, 22);
        txt.draw(out, &name, popup.title[0] + 8.0 * s, txt.baseline_in(popup.title, title_size), title_size, pal::TEXT);
        for (index, (&bounds, &action)) in popup.buttons.iter().zip(&menu.actions).enumerate() {
            let lit = hovered == Some(ScoreboardHit::Action(index));
            let tone = match action {
                ScoreAction::Mute | ScoreAction::Unmute => pal::ACCENT,
                ScoreAction::VoteKick | ScoreAction::ForceSpectate => pal::ROSE_LIGHT,
            };
            if lit {
                rect(out, bounds[0], bounds[1], bounds[2], bounds[3], pal::fade(tone, 0.22), w, h);
                rect(out, bounds[0], bounds[1], (3.0 * s).max(2.0), bounds[3], tone, w, h);
            }
            let size = px(size::CAPTION) * 1.1;
            txt.draw(out, action.label(), bounds[0] + 14.0 * s, txt.baseline_in(bounds, size), size, if lit { pal::TEXT } else { tone });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_half(
    out: &mut Vec<UiVertex>,
    board: &UiScoreboard,
    ui: &UiSnapshot,
    half: &Half,
    layout: &BoardLayout,
    hovered: Option<ScoreboardHit>,
    menu_client: Option<i32>,
    txt: &UiText,
    w: u32,
    h: u32,
) {
    let s = layout.s;
    let tone = team_tone(half.team);
    let [hx, hy, hw, hh] = half.panel;
    // A light team wash over the whole half, a stronger band for its header.
    rect(out, hx, hy, hw, hh, pal::fade(tone, 0.07), w, h);
    let [bx, by, bw, bh] = half.header;
    let (l, r) = (pal::fade(tone, 0.42), pal::fade(tone, 0.10));
    rect_gradient(out, bx, by, bw, bh, l, r, l, r, w, h);
    rect(out, bx, by + bh - 1.0, bw, (1.5 * s).max(1.0), pal::fade(tone, 0.8), w, h);
    let title = match (half.team, half.side) {
        (1, _) => "RED TEAM".to_owned(),
        (2, _) => "BLUE TEAM".to_owned(),
        _ => "PLAYERS".to_owned(),
    };
    let count = half.members.len();
    let heading = format!("{title}   ·   {count}");
    let size = size::HALF_TITLE * s;
    let baseline = txt.baseline_in(half.header, size);
    let caption = size::CAPTION * s;
    // The title starts at the row's left edge, over the avatars.
    let title_x = half.slots.first().map_or(bx, |slot| slot.1) + 4.0 * s;
    txt.draw(out, &heading, title_x, baseline, size, pal::TEXT);
    for &(slot, x, width) in &half.slots {
        match slot {
            Slot::Name | Slot::Avatar => {}
            _ => {
                let label = slot.caption();
                if !label.is_empty() {
                    txt.draw_centered(out, label, [x, by, width, bh], caption, pal::fade(pal::TEXT, 0.75));
                }
            }
        }
    }

    for &(bounds, index) in &half.rows {
        let Some(entry) = board.entries.get(index) else { continue };
        let lit = hovered == Some(ScoreboardHit::Row(entry.client)) || menu_client == Some(entry.client);
        draw_row(out, board, ui, entry, half, bounds, layout, lit, txt, w, h);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_row(
    out: &mut Vec<UiVertex>,
    board: &UiScoreboard,
    ui: &UiSnapshot,
    entry: &UiScoreEntry,
    half: &Half,
    bounds: Bounds,
    layout: &BoardLayout,
    lit: bool,
    txt: &UiText,
    w: u32,
    h: u32,
) {
    let s = layout.s;
    let [rx, ry, rw, rh] = bounds;
    let row = size::ROW * s;
    let cap = txt.cap(row);
    let baseline = ry + (rh + cap) * 0.5;
    let tone = team_tone(entry.team);
    // You (or whoever you follow) in gold, so the row never blends into its
    // team wash; the pointer/menu row gets a neutral light outline instead.
    let focused = ui.scoreboard_focus_client == Some(entry.client);
    let (bx, by, bw, bh) = (rx + 4.0 * s, ry + 2.0 * s, rw - 8.0 * s, rh - 3.0 * s);
    if focused {
        let strong = pal::fade(pal::GOLD, 0.30);
        let faint = pal::fade(pal::GOLD, 0.06);
        rect_gradient(out, bx, by, bw, bh, strong, faint, strong, faint, w, h);
        rect_outline(out, bx, by, bw, bh, (2.0 * s).max(1.5), pal::fade(pal::GOLD, 0.95), w, h);
        rect(out, bx, by, 5.0 * s, bh, pal::GOLD, w, h);
    }
    if lit {
        rect(out, bx, by, bw, bh, pal::fade(pal::TEXT, 0.08), w, h);
        rect_outline(out, bx, by, bw, bh, (1.0 * s).max(1.0), pal::fade(pal::TEXT, 0.7), w, h);
    }
    rect(out, rx + 12.0 * s, ry + rh - 1.0, rw - 24.0 * s, 1.0, pal::fade(pal::RULE, 0.35), w, h);
    rect(out, rx + 4.0 * s, ry + rh * 0.24, (3.0 * s).max(2.0), rh * 0.52, pal::fade(tone, 0.9), w, h);

    let center = |x: f32, width: f32| [x, ry, width, rh];
    for &(slot, x, width) in &half.slots {
        match slot {
            Slot::Name => {
                let mut chars = 32usize;
                let mut name = truncate_jka_text(&entry.name, chars);
                let room = width - 30.0 * s;
                while chars > 6 && txt.width(&name, row) > room {
                    chars -= 2;
                    name = truncate_jka_text(&entry.name, chars);
                }
                let name_w = txt.width(&name, row);
                let name_x = x + 6.0 * s;
                let color = if entry.ping == -1 { pal::TEXT_FAINT } else { pal::TEXT };
                txt.draw(out, &name, name_x, baseline, row, color);
                if entry.crowned {
                    let crown = glyphs::crown_over([name_x, baseline - cap, name_w, cap], s);
                    glyphs::crown(out, crown, 0.92, w, h);
                }
                // Badges follow the name.
                let mut badge_x = name_x + name_w + 8.0 * s;
                let mut badge = |out: &mut Vec<UiVertex>, width_px: f32, draw: &dyn Fn(&mut Vec<UiVertex>, f32)| {
                    draw(out, badge_x);
                    badge_x += width_px + 6.0 * s;
                };
                if entry.muted {
                    let icon = 14.0 * s;
                    badge(out, icon * 1.15, &|out, x0| glyphs::muted(out, [x0, ry + (rh - icon) * 0.5, icon * 1.15, icon], 1.0, w, h));
                }
                let ready = (0..16).contains(&entry.client) && (board.ready_clients & (1u16 << entry.client as u32)) != 0;
                if ready {
                    let small = size::SMALL * s;
                    let tw = txt.width("READY", small);
                    badge(out, tw, &|out, x0| txt.draw(out, "READY", x0, ry + (rh + txt.cap(small)) * 0.5, small, pal::ACCENT));
                }
                if entry.spectating_you {
                    let small = size::SMALL * s;
                    let tw = txt.width("WATCHING YOU", small);
                    badge(out, tw, &|out, x0| txt.draw(out, "WATCHING YOU", x0, ry + (rh + txt.cap(small)) * 0.5, small, pal::ACCENT));
                }
                if board.siege {
                    let icon = 24.0 * s;
                    let cell = entry.siege_icon.and_then(|i| u16::try_from(scoreboard_icon_atlas_offset(ui) + usize::from(i)).ok());
                    let count = shared_player_icon_count(ui);
                    badge(out, icon, &|out, x0| player_icon_atlas_cell(out, count, cell, x0, ry + (rh - icon) * 0.5, icon, icon, w, h));
                }
            }
            Slot::Score => {
                let value = if entry.ping == -1 && board.ctf { "-".to_owned() } else { entry.score.to_string() };
                txt.draw_centered(out, &value, center(x, width), row, pal::TEXT);
            }
            Slot::Deaths => {
                if let Some(deaths) = entry.deaths {
                    txt.draw_centered(out, &deaths.to_string(), center(x, width), row, pal::TEXT_DIM);
                }
            }
            Slot::Captures => txt.draw_centered(out, &entry.captures.to_string(), center(x, width), row, pal::TEXT_DIM),
            Slot::Accuracy => txt.draw_centered(out, &format!("{}%", entry.accuracy.max(0)), center(x, width), size::SMALL * s * 1.1, pal::TEXT_DIM),
            Slot::Excellent | Slot::Baton | Slot::Defends | Slot::Assists => {
                let value = slot.counter(entry).unwrap_or(0).max(0);
                let color = if value > 0 { pal::TEXT_DIM } else { pal::TEXT_FAINT };
                txt.draw_centered(out, &value.to_string(), center(x, width), size::SMALL * s * 1.1, color);
            }
            Slot::Time => {
                // `scores` time is whole minutes on the server.
                if entry.ping >= 0 {
                    txt.draw_centered(out, &format!("{}m", entry.time.max(0)), center(x, width), size::SMALL * s * 1.1, pal::TEXT_DIM);
                }
            }
            Slot::Ping => {
                let (value, color) = if entry.ping < 0 {
                    ("CNCT".to_owned(), pal::TEXT_FAINT)
                } else if entry.ping <= 60 {
                    (entry.ping.to_string(), pal::ACCENT)
                } else if entry.ping <= 120 {
                    (entry.ping.to_string(), pal::GOLD)
                } else {
                    (entry.ping.to_string(), pal::ROSE_LIGHT)
                };
                txt.draw_centered(out, &value, center(x, width), size::SMALL * s * 1.1, color);
            }
            Slot::HeadToHead => {
                let Some((kills, deaths)) = entry.h2h else { continue };
                let small = size::SMALL * s * 1.1;
                let text = format!("{kills}/{deaths}");
                let tw = txt.width(&text, small);
                let cup = 14.0 * s;
                let total = cup + 5.0 * s + tw;
                let start = x + (width - total) * 0.5;
                let text_color = if kills > deaths { pal::GOLD } else if kills < deaths { pal::ROSE_LIGHT } else { pal::TEXT_DIM };
                let cup_y = ry + (rh - cup) * 0.5;
                glyphs::cup(out, [start, cup_y, cup, cup], 0.95, w, h);
                txt.draw(out, &text, start + cup + 5.0 * s, ry + (rh + txt.cap(small)) * 0.5, small, text_color);
            }
            Slot::Avatar => {
                let Some(atlas) = ui.avatar_atlas.as_ref() else { continue };
                // A model that is not installed shows the stand-in the game
                // draws for it, dimmed so it reads as "not the real one".
                let Some(((uv0, uv1), alpha)) = atlas.uv(&entry.model).map(|uv| (uv, 0.78))
                    .or_else(|| atlas.uv(&entry.model_fallback).map(|uv| (uv, 0.38))) else { continue };
                let radius = (rh * 0.5 - 2.0 * s).min(width * 0.5);
                glyphs::avatar(out, [x + width * 0.5, ry + rh * 0.5], radius, uv0, uv1, alpha, w, h);
            }
        }
    }
}

/// The personal match breakdown: headline numbers and averages on the left,
/// every opponent faced on the right.
fn draw_stats(out: &mut Vec<UiVertex>, stats: &UiMatchStats, body: Bounds, s: f32, txt: &UiText, w: u32, h: u32) {
    let [bx, by, bw, bh] = body;
    rect(out, bx, by, bw, bh, pal::fade(pal::SLAB_BOTTOM, 0.55), w, h);
    let pad = 28.0 * s;
    let mut flex = Flex::new();
    let left = flex.spring();
    let right = flex.spring();
    let row = flex.node(Container::row().width(bw).height(bh).pad(pad, 18.0 * s).gap(36.0 * s).align(AlignItems::STRETCH), &[left, right]);
    flex.compute(row, bw, bh);
    let at = |b: Bounds| [bx + b[0], by + b[1], b[2], b[3]];
    let (left, right) = (at(flex.bounds(left)), at(flex.bounds(right)));
    let caption = size::CAPTION * s;
    let value = size::STAT_VALUE * s;

    // Headline tiles: four to a line.
    let kdr = if stats.deaths == 0 { stats.kills as f32 } else { stats.kills as f32 / stats.deaths as f32 };
    let tiles: [(&str, String, [f32; 4]); 8] = [
        ("KILLS", stats.kills.to_string(), pal::ACCENT),
        ("DEATHS", stats.deaths.to_string(), pal::ROSE_LIGHT),
        ("K / D", format!("{kdr:.2}"), pal::GOLD),
        ("BEST STREAK", stats.best_streak.to_string(), pal::TEXT),
        ("AIR KILLS", stats.aerial_kills.to_string(), pal::TEXT),
        ("SABER KILLS", stats.saber_kills.to_string(), pal::TEXT),
        ("HITS LANDED", stats.hits.to_string(), pal::TEXT),
        ("DAMAGE TAKEN", stats.damage_taken.to_string(), pal::TEXT),
    ];
    let tile_w = left[2] / 4.0;
    let tile_h = 70.0 * s;
    for (i, (label, number, color)) in tiles.iter().enumerate() {
        let (col, line) = ((i % 4) as f32, (i / 4) as f32);
        let tile = [left[0] + col * tile_w, left[1] + line * (tile_h + 8.0 * s), tile_w - 8.0 * s, tile_h];
        rect(out, tile[0], tile[1], tile[2], tile[3], pal::fade(pal::TEXT, 0.04), w, h);
        rect(out, tile[0], tile[1], (2.0 * s).max(1.0), tile[3], pal::fade(*color, 0.8), w, h);
        txt.draw(out, label, tile[0] + 12.0 * s, tile[1] + 10.0 * s + txt.cap(caption), caption, pal::TEXT_FAINT);
        txt.draw(out, number, tile[0] + 12.0 * s, tile[1] + tile[3] - 12.0 * s, value, *color);
    }
    // Averages and team totals as label / value lines.
    let mut y = left[1] + 2.0 * (tile_h + 8.0 * s) + 18.0 * s;
    let line_h = 30.0 * s;
    let small = size::META * s;
    let mut line = |out: &mut Vec<UiVertex>, label: &str, value: String, color: [f32; 4]| {
        txt.draw(out, label, left[0], y + txt.cap(small), small, pal::TEXT_DIM);
        txt.draw_right(out, &value, left[0] + left[2] - 8.0 * s, y + txt.cap(small), small, color);
        rect(out, left[0], y + line_h - 8.0 * s, left[2] - 8.0 * s, 1.0, pal::fade(pal::RULE, 0.4), w, h);
        y += line_h;
    };
    let or_dash = |v: Option<String>| v.unwrap_or_else(|| "-".to_owned());
    line(out, "Force spent per kill", or_dash(stats.avg_force_per_kill.map(|v| format!("{v} pts"))), pal::TEXT);
    line(out, "Time to kill from first sight", or_dash(stats.avg_ttk_ms.map(|v| format!("{:.1} s", v as f32 / 1000.0))), pal::TEXT);
    line(out, "Damage taken per death", or_dash(stats.avg_damage_per_death.map(|v| v.to_string())), pal::TEXT);
    if stats.suicides > 0 {
        line(out, "Suicides and falls", stats.suicides.to_string(), pal::ROSE_LIGHT);
    }
    if let Some(accuracy) = stats.accuracy {
        line(out, "Accuracy (server)", format!("{accuracy}%"), pal::TEXT);
    }
    if stats.excellent > 0 {
        line(out, "Excellent: two kills in 3 s", stats.excellent.to_string(), pal::GOLD);
    }
    if stats.gauntlet > 0 {
        line(out, "Stun baton kills", stats.gauntlet.to_string(), pal::GOLD);
    }
    if stats.team_kills.iter().chain(&stats.team_deaths).any(|&v| v > 0) {
        line(out, "Kills  RED / BLUE", format!("{} / {}", stats.team_kills[0], stats.team_kills[1]), pal::TEXT);
        line(out, "Deaths  RED / BLUE", format!("{} / {}", stats.team_deaths[0], stats.team_deaths[1]), pal::TEXT);
    }

    // Matchups: best and worst first, then everyone faced.
    let mut y = right[1];
    let head = size::HALF_TITLE * s;
    txt.draw(out, "MATCHUPS", right[0], y + txt.cap(head), head, pal::TEXT);
    y += 30.0 * s;
    for (label, who, color) in [("BEST", &stats.best, pal::GOLD), ("NEMESIS", &stats.nemesis, pal::ROSE_LIGHT)] {
        let Some(name) = who else { continue };
        txt.draw(out, label, right[0], y + txt.cap(caption), caption, color);
        txt.draw(out, &truncate_jka_text(name, 26), right[0] + 90.0 * s, y + txt.cap(caption), caption, pal::TEXT);
        y += 24.0 * s;
    }
    y += 8.0 * s;
    let columns = [("OPPONENT", 0.0), ("YOU : THEM", 0.46), ("WIN %", 0.68), ("DMG TAKEN", 0.84)];
    for (label, at) in columns {
        txt.draw(out, label, right[0] + right[2] * at, y + txt.cap(caption), caption, pal::TEXT_FAINT);
    }
    y += 24.0 * s;
    let row_h = 30.0 * s;
    for matchup in &stats.matchups {
        if y + row_h > right[1] + right[3] {
            break;
        }
        let tone = team_tone(matchup.team);
        rect(out, right[0], y + 6.0 * s, (3.0 * s).max(2.0), row_h - 12.0 * s, pal::fade(tone, 0.8), w, h);
        let base = y + (row_h + txt.cap(small)) * 0.5;
        txt.draw(out, &truncate_jka_text(&matchup.name, 22), right[0] + 10.0 * s, base, small, pal::TEXT);
        let cup = 13.0 * s;
        let x = right[0] + right[2] * 0.46;
        if matchup.kills > matchup.deaths {
            glyphs::cup(out, [x, y + (row_h - cup) * 0.5, cup, cup], 0.95, w, h);
        }
        let score_color = if matchup.kills > matchup.deaths { pal::GOLD } else if matchup.kills < matchup.deaths { pal::ROSE_LIGHT } else { pal::TEXT_DIM };
        txt.draw(out, &format!("{} : {}", matchup.kills, matchup.deaths), x + cup + 6.0 * s, base, small, score_color);
        let total = u32::from(matchup.kills) + u32::from(matchup.deaths);
        let win = if total == 0 { "-".to_owned() } else { format!("{}%", u32::from(matchup.kills) * 100 / total) };
        txt.draw(out, &win, right[0] + right[2] * 0.68, base, small, pal::TEXT_DIM);
        txt.draw(out, &matchup.damage_taken.to_string(), right[0] + right[2] * 0.84, base, small, pal::TEXT_DIM);
        rect(out, right[0], y + row_h - 1.0, right[2], 1.0, pal::fade(pal::RULE, 0.35), w, h);
        y += row_h;
    }
    if stats.matchups.is_empty() {
        txt.draw(out, "Nobody faced yet.", right[0], y + txt.cap(small), small, pal::TEXT_FAINT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{ScoreboardChrome, ScoreboardCursorUi};

    fn entry(client: i32, team: i32, score: i32) -> UiScoreEntry {
        UiScoreEntry {
            client,
            name: format!("Player{client}"),
            score,
            powerups: 0,
            captures: 0,
            assists: 0,
            defends: 0,
            deaths: None,
            ping: 40,
            time: 3,
            team,
            siege_class: String::new(),
            siege_icon: None,
            spectating_you: false,
            muted: false,
            crowned: false,
            model: String::new(),
            model_fallback: String::new(),
            h2h: None,
            accuracy: 0,
            excellent: 0,
            gauntlet: 0,
        }
    }

    fn board(entries: Vec<UiScoreEntry>, team_game: bool) -> UiScoreboard {
        UiScoreboard {
            team_scores: [3, 1],
            ready_clients: 0,
            team_game,
            ctf: false,
            siege: false,
            siege_win_team: 0,
            spectator_scores: false,
            show_deaths: false,
            icon_paths: Vec::new(),
            entries,
            chrome: ScoreboardChrome { avatars: true, head_to_head: true, ..ScoreboardChrome::default() },
        }
    }

    #[test]
    fn teams_split_left_red_right_blue_and_ffa_splits_in_half() {
        let teams = board(vec![entry(0, 2, 1), entry(1, 1, 5), entry(2, 3, 0), entry(3, 1, 2)], true);
        let ([left, right], spectators, sides) = split(&teams);
        assert_eq!((left, right, spectators, sides), (vec![1, 3], vec![0], vec![2], [1, 2]));
        let ffa = board((0..5).map(|i| entry(i, 0, i)).collect(), false);
        let ([left, right], _, _) = split(&ffa);
        assert_eq!((left.len(), right.len()), (3, 2));
    }

    #[test]
    fn both_halves_read_avatar_name_score_then_ping_and_time_last() {
        let mut board = board((0..6).map(|i| entry(i, 0, i)).collect(), false);
        board.show_deaths = true;
        board.entries[2].excellent = 1;
        let layout = board_layout(&board, 1920, 1080);
        for half in &layout.halves {
            let kinds: Vec<Slot> = half.slots.iter().map(|slot| slot.0).collect();
            assert_eq!(
                kinds,
                [Slot::Avatar, Slot::Name, Slot::Score, Slot::Deaths, Slot::HeadToHead, Slot::Excellent, Slot::Ping, Slot::Time]
            );
            let time = half.slots.last().unwrap();
            assert!(time.1 + time.2 <= half.panel[0] + half.panel[2] + 0.5);
        }
    }

    #[test]
    fn a_full_server_scrolls_both_halves_together() {
        let mut board = board((0..64).map(|i| entry(i, 0, i)).collect(), false);
        let layout = board_layout(&board, 1920, 1080);
        assert!(layout.max_scroll > 0 && layout.scrollbar.is_some());
        board.chrome.scroll = u16::MAX;
        let end = board_layout(&board, 1920, 1080);
        assert_eq!(end.halves[0].rows.last().unwrap().1, 31);
        assert_eq!(end.halves[1].rows.last().unwrap().1, 63);
    }

    #[test]
    fn hit_test_finds_rows_in_either_half_and_spectators() {
        let board = board(vec![entry(10, 1, 3), entry(11, 2, 1), entry(12, 3, 0)], true);
        let layout = board_layout(&board, 1920, 1080);
        let center = |b: Bounds| [b[0] + b[2] * 0.5, b[1] + b[3] * 0.5];
        assert_eq!(hit_test(&board, 1920, 1080, center(layout.halves[0].rows[0].0)), Some(ScoreboardHit::Row(10)));
        assert_eq!(hit_test(&board, 1920, 1080, center(layout.halves[1].rows[0].0)), Some(ScoreboardHit::Row(11)));
        assert_eq!(hit_test(&board, 1920, 1080, center(layout.spectators[0].0)), Some(ScoreboardHit::Row(12)));
        assert_eq!(hit_test(&board, 1920, 1080, [2.0, 2.0]), None);
    }

    #[test]
    fn open_menu_takes_clicks_before_rows() {
        let mut board = board((0..4).map(|i| entry(i, 0, i)).collect(), false);
        let row = board_layout(&board, 1920, 1080).halves[0].rows[0].0;
        let anchor = [(row[0] + 200.0) as i32, (row[1] + 4.0) as i32];
        let menu = ScoreboardMenuUi { client: 0, name: "Player0".into(), anchor, actions: vec![ScoreAction::Mute, ScoreAction::VoteKick] };
        let popup = menu_layout(&menu, 1920, 1080);
        board.chrome.cursor = Some(ScoreboardCursorUi { pointer: None, hovered: None, menu: Some(menu) });
        let button = popup.buttons[1];
        assert_eq!(hit_test(&board, 1920, 1080, [button[0] + 4.0, button[1] + 4.0]), Some(ScoreboardHit::Action(1)));
    }

    #[test]
    fn stats_toggle_and_vote_buttons_are_clickable() {
        let mut board = board(vec![entry(0, 0, 0)], false);
        board.chrome.stats = Some(UiMatchStats::default());
        board.chrome.vote = Some("Kick Player".into());
        let layout = board_layout(&board, 1920, 1080);
        let toggle = layout.stats_toggle;
        assert_eq!(hit_test(&board, 1920, 1080, [toggle[0] + 2.0, toggle[1] + 2.0]), Some(ScoreboardHit::StatsToggle));
        let yes = layout.vote_yes.unwrap();
        assert_eq!(hit_test(&board, 1920, 1080, [yes[0] + 1.0, yes[1] + 1.0]), Some(ScoreboardHit::VoteYes));
        board.chrome.stats_open = true;
        let open = board_layout(&board, 1920, 1080);
        assert!(open.stats_open && open.max_scroll == 0);
    }
}
