//! cg_specHud 2-4: observer HUD for team games, after the CS2 / CS:GO
//! broadcast layout. Taffy (`ui::flex`) places three groups:
//! - a top match bar: team blocks with alive pips, score boxes, centre clock;
//! - player card stacks, RED bottom-left and BLUE mirrored bottom-right,
//!   each card led by the player's avatar (the scoreboard's icon atlas);
//! - cg_specHudMap: a top-down radar of the map, top-left (`broadcast_map`);
//! The observed player's card is outlined; while it (or the free camera) is
//! up, the native health/shield/ammo/Force meters step aside.
//! Health, armor and Force only ever come from observed playerStates; unknown
//! values are drawn as unknown and old samples are dimmed.
use super::broadcast_map::{self, RadarBlip, RadarView};
use super::glyphs::AVATAR_SOURCE;
use super::{flag_carrier_icon, flag_status_icon, EXT_IMAGE_SHADER_SOURCE_BASE, EXT_IMAGE_TEXTURE_SOURCE};
use crate::cgame::{TEAM_BLUE, TEAM_RED};
use crate::ui::flex::{Bounds, Container, Flex};
use glam::{Vec2, Vec3};
use crate::ui::hud::movement::{ICON_WEAPON_BASE, WEAPON_ICON_NAMES};
use crate::ui::text::UiText;
use crate::ui::{
    rect, rect_gradient, rect_outline, textured_rect_with_source, BroadcastPlayerUi,
    BroadcastVitals, ProportionalFont, SpectatorBroadcastUi, UiSnapshot, UiVertex,
};
use taffy::prelude::{AlignItems, JustifyContent, NodeId};

const PLATE: [f32; 4] = [0.047, 0.055, 0.071, 0.84];
const PLATE_DEAD: [f32; 4] = [0.035, 0.038, 0.045, 0.66];
const PLATE_DEEP: [f32; 4] = [0.020, 0.024, 0.032, 0.92];
const INK: [f32; 4] = [0.95, 0.96, 0.98, 1.0];
const MUTED: [f32; 4] = [0.60, 0.64, 0.70, 1.0];
const FAINT: [f32; 4] = [0.36, 0.39, 0.44, 1.0];
const RED: [f32; 4] = [0.89, 0.31, 0.27, 1.0];
const BLUE: [f32; 4] = [0.36, 0.58, 0.95, 1.0];
const HEALTH_LOW: [f32; 4] = [0.96, 0.33, 0.22, 1.0];
const FORCE: [f32; 4] = [0.45, 0.68, 1.0, 1.0];
/// Card health/armor pair, after the native HUD's health (red) and shield
/// (green) meters.
const HP_RED: [f32; 4] = [0.96, 0.36, 0.33, 1.0];
const AR_GREEN: [f32; 4] = [0.42, 0.88, 0.48, 1.0];
/// The lost chunk behind an animated bar before it drains.
const TRAIL: [f32; 4] = [1.0, 0.93, 0.86, 1.0];
/// Samples older than about two sweep intervals are drawn dimmed.
const STALE_MS: u32 = 5_000;

#[derive(Clone, Copy)]
struct Side {
    team: i32,
    color: [f32; 4],
    label: &'static str,
    /// BLUE mirrors every row so the HP box sits on the outer screen edge.
    mirrored: bool,
}

const SIDES: [Side; 2] = [
    Side { team: TEAM_RED, color: RED, label: "RED", mirrored: false },
    Side { team: TEAM_BLUE, color: BLUE, label: "BLUE", mirrored: true },
];

/// Use the *same* shaders as JK2's flag-status HUD and the JKA Siege
/// scoreboard, including the red/blue textures (not colored vector flags).
pub(crate) fn append_broadcast_flag_images(images: &mut Vec<String>) {
    // JKA only needs the plain scoreboard flags; JK2 already registers the
    // *_x taken versions through append_jk2_ctf_images.
    for team in [TEAM_RED, TEAM_BLUE] {
        let Some(name) = flag_carrier_icon(team) else { continue };
        if !images.iter().any(|present| present == name) {
            images.push(name.to_owned());
        }
    }
}

fn fade(color: [f32; 4], alpha: f32) -> [f32; 4] {
    [color[0], color[1], color[2], color[3] * alpha]
}

fn strip_colors(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^' && chars.peek().is_some_and(|next| next.is_ascii_alphanumeric()) {
            chars.next();
            continue;
        }
        out.push(c);
    }
    out
}

fn game_type_label(game_type: i32) -> &'static str {
    match game_type {
        6 => "TEAM FFA",
        7 => "SIEGE",
        8 => "CTF",
        9 => "CTY",
        _ => "TEAMS",
    }
}

fn has_weapon_icon(weapon: Option<i32>) -> Option<usize> {
    let weapon = usize::try_from(weapon?).ok()?;
    WEAPON_ICON_NAMES.get(weapon).filter(|name| !name.is_empty()).map(|_| weapon)
}

/// Health fill fraction against STAT_MAX_HEALTH (100 when unknown).
fn health_fraction(vitals: &BroadcastVitals) -> Option<f32> {
    let health = vitals.health?;
    let max = vitals.max_health.filter(|max| *max > 0).unwrap_or(100);
    Some((health.max(0) as f32 / max as f32).clamp(0.0, 1.0))
}

struct Paint<'a> {
    txt: UiText<'a>,
    ui: &'a UiSnapshot,
    w: u32,
    h: u32,
    s: f32,
}

impl Paint<'_> {
    fn fill(&self, out: &mut Vec<UiVertex>, b: Bounds, color: [f32; 4]) {
        rect(out, b[0], b[1], b[2], b[3], color, self.w, self.h);
    }

    /// Horizontal gradient from `from` (left) to `to` (right).
    fn ramp(&self, out: &mut Vec<UiVertex>, b: Bounds, from: [f32; 4], to: [f32; 4]) {
        rect_gradient(out, b[0], b[1], b[2], b[3], from, to, from, to, self.w, self.h);
    }

    fn outline(&self, out: &mut Vec<UiVertex>, b: Bounds, thickness: f32, color: [f32; 4]) {
        rect_outline(out, b[0], b[1], b[2], b[3], thickness, color, self.w, self.h);
    }

    fn text(&self, out: &mut Vec<UiVertex>, value: &str, b: Bounds, px: f32, color: [f32; 4]) {
        self.txt.draw(out, value, b[0], self.txt.baseline_in(b, px), px, color);
    }

    fn text_centered(&self, out: &mut Vec<UiVertex>, value: &str, b: Bounds, px: f32, color: [f32; 4]) {
        self.txt.draw_centered(out, value, b, px, color);
    }

    /// Fixed-size leaf sized to `value` at `px`.
    fn text_leaf(&self, flex: &mut Flex, value: &str, px: f32) -> NodeId {
        flex.leaf(self.txt.width(value, px), self.txt.cap(px))
    }

    /// `value` cut (on a char boundary) to fit `max_w`, with a ".." tail.
    fn fit(&self, value: &str, px: f32, max_w: f32) -> String {
        if self.txt.width(value, px) <= max_w {
            return value.to_owned();
        }
        let mut cut = value.to_owned();
        while !cut.is_empty() {
            cut.pop();
            let candidate = format!("{cut}^7..");
            if self.txt.width(&candidate, px) <= max_w {
                return candidate;
            }
        }
        String::new()
    }

    fn flag_icon(&self, out: &mut Vec<UiVertex>, flag_team: i32, b: Bounds) {
        if let Some(name) = flag_carrier_icon(flag_team) {
            self.hud_image(out, name, b);
        }
    }

    fn hud_image(&self, out: &mut Vec<UiVertex>, name: &str, b: Bounds) {
        let Some(index) = self.ui.extension_images.iter().position(|image| image == name) else { return };
        let shader = self.ui.extension_shader_flags.get(index).copied().unwrap_or(false);
        let (uv0, uv1) = self.ui.extension_uv(index);
        if !shader && uv0 == uv1 { return; } // Asset not yet loaded.
        textured_rect_with_source(out, b[0], b[1], b[2], b[3], uv0, uv1, INK,
            if shader { EXT_IMAGE_SHADER_SOURCE_BASE + index as f32 }
            else { EXT_IMAGE_TEXTURE_SOURCE }, self.w, self.h);
    }

    fn weapon_icon(&self, out: &mut Vec<UiVertex>, weapon: usize, b: Bounds, tint: [f32; 4]) {
        let index = ICON_WEAPON_BASE + weapon;
        let (uv0, uv1) = self.ui.icon_uv(index);
        // Weapon art is wider than tall; keep its aspect inside the box.
        let size = b[3].min(b[2]);
        let x = b[0] + (b[2] - size) * 0.5;
        textured_rect_with_source(out, x, b[1], size, size, uv0, uv1, tint,
            self.ui.icon_texture_source(index), self.w, self.h);
    }
}

pub(super) fn draw(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    let Some(broadcast) = ui.spectator_broadcast.as_ref() else { return };
    if broadcast.players.is_empty() {
        return;
    }
    // One scale for both axes against a 1920x1080 reference: every group is
    // authored in reference units, so the layout fits any aspect ratio.
    let s = (h as f32 / 1080.0).min(w as f32 / 1920.0).clamp(0.5, 2.5);
    let p = Paint { txt: UiText::new(ui, small_font, w, h), ui, w, h, s };
    let bar_bottom = top_bar(out, &p, broadcast);
    // Broadcast owns the status HUD in modes 2-4, including probe frames.
    // Keep a single bottom anchor; telemetry may change but layout never does.
    let stack_bottom = h as f32 - 18.0 * s;
    let stack_top = bar_bottom + 24.0 * s;
    // The radar holds the top-left corner; RED's stack starts below it.
    let red_top = radar(out, &p, broadcast).map_or(stack_top, |bottom| stack_top.max(bottom + 16.0 * s));
    player_stacks(out, &p, broadcast, [red_top, stack_top], stack_bottom);
}

fn side_color(team: i32) -> Option<[f32; 4]> {
    SIDES.iter().find(|side| side.team == team).map(|side| side.color)
}

/// cg_specHudMap radar, top-left. Returns the panel's bottom edge.
fn radar(out: &mut Vec<UiVertex>, p: &Paint, b: &SpectatorBroadcastUi) -> Option<f32> {
    if b.map_mode == 0 {
        return None;
    }
    let map = b.minimap.as_ref()?.0.as_ref();
    if map.is_empty() {
        return None;
    }
    let s = p.s;
    let world = |position: [i32; 3]| Vec3::new(position[0] as f32, position[1] as f32, position[2] as f32);
    let observed_at = b.observed.as_ref()
        .and_then(|observed| b.players.iter().find(|player| player.client == observed.client))
        .and_then(|player| player.position)
        .or(b.camera.map(|(position, _)| position));
    let view = match (b.map_mode, observed_at) {
        (2, Some(position)) => RadarView::Follow(world(position).truncate()),
        _ => RadarView::Overview,
    };
    let pad = 8.0 * s;
    let size = broadcast_map::panel_size(map, view, Vec2::new(300.0 * s, 280.0 * s));
    let outer = [16.0 * s, 14.0 * s, size.x + pad * 2.0, size.y + pad * 2.0];
    p.fill(out, outer, PLATE);
    p.fill(out, [outer[0], outer[1], outer[2], 2.0 * s], fade(INK, 0.14));
    let inner = [outer[0] + pad, outer[1] + pad, size.x, size.y];
    let frame = broadcast_map::draw_floor(out, map, inner, view, s, p.w, p.h);

    let observed = b.observed.as_ref().map(|observed| observed.client);
    let mut blips: Vec<RadarBlip> = b.players.iter()
        .filter_map(|player| {
            let position = player.position?;
            Some(RadarBlip {
                position: world(position),
                yaw: f32::from(player.yaw),
                color: side_color(player.team)?,
                observed: observed == Some(player.client),
                alive: player.alive != Some(false),
                stale: player.position_stale,
                flag: player.carrying_flag.and_then(side_color),
                camera: false,
            })
        })
        .collect();
    if let Some((position, yaw)) = b.camera.filter(|_| observed.is_none()) {
        blips.push(RadarBlip {
            position: world(position),
            yaw: f32::from(yaw),
            color: INK,
            observed: false,
            alive: true,
            stale: false,
            flag: None,
            camera: true,
        });
    }
    broadcast_map::draw_blips(out, &frame, &blips, s, p.w, p.h);
    Some(outer[1] + outer[3])
}

/// CS2 match bar: [RED block][score][clock][score][BLUE block].
fn top_bar(out: &mut Vec<UiVertex>, p: &Paint, b: &SpectatorBroadcastUi) -> f32 {
    let s = p.s;
    let (w, h) = (p.w as f32, p.h as f32);
    let bar_h = 56.0 * s;
    let team_w = 236.0 * s;
    let score_w = 74.0 * s;
    let center_w = 140.0 * s;
    let name_px = 22.0 * s;
    let small_px = 12.0 * s;
    let score_px = 36.0 * s;
    let clock_px = 26.0 * s;

    let mut flex = Flex::new();
    struct TeamNodes {
        block: NodeId,
        name: NodeId,
        count: NodeId,
        pips: Vec<(NodeId, bool)>,
        score: NodeId,
        score_box: NodeId,
        count_text: String,
    }
    let mut teams = Vec::new();
    for side in SIDES {
        let players: Vec<&BroadcastPlayerUi> = b.players.iter().filter(|pl| pl.team == side.team).collect();
        let alive = players.iter().filter(|pl| pl.alive != Some(false)).count();
        let count_text = format!("{alive} / {} ALIVE", players.len());
        let name = p.text_leaf(&mut flex, side.label, name_px);
        let count = p.text_leaf(&mut flex, &count_text, small_px);
        let spring = flex.spring();
        let head_children = if side.mirrored { [count, spring, name] } else { [name, spring, count] };
        let head = flex.node(Container::row(), &head_children);
        let pips: Vec<(NodeId, bool)> = players.iter().take(16)
            .map(|pl| (flex.leaf(14.0 * s, 4.0 * s), pl.alive != Some(false)))
            .collect();
        let pip_nodes: Vec<NodeId> = pips.iter().map(|(node, _)| *node).collect();
        let pip_row = flex.node(
            Container::row().gap(3.0 * s)
                .justify(if side.mirrored { JustifyContent::FLEX_END } else { JustifyContent::FLEX_START }),
            &pip_nodes,
        );
        let block = flex.node(
            Container::column().width(team_w).height(bar_h).pad(14.0 * s, 10.0 * s)
                .gap(9.0 * s).justify(JustifyContent::CENTER),
            &[head, pip_row],
        );
        let score_text = b.team_scores[if side.mirrored { 1 } else { 0 }].to_string();
        let score = p.text_leaf(&mut flex, &score_text, score_px);
        let score_box = flex.node(
            Container::row().width(score_w).height(bar_h).justify(JustifyContent::CENTER),
            &[score],
        );
        teams.push(TeamNodes { block, name, count, pips, score, score_box, count_text });
    }
    let clock_text = b.clock.clone().unwrap_or_else(|| "--:--".to_owned());
    let sub_text = match b.score_limit.filter(|limit| *limit > 0) {
        Some(limit) => format!("{}   FIRST TO {limit}", game_type_label(b.game_type)),
        None => game_type_label(b.game_type).to_owned(),
    };
    let clock = p.text_leaf(&mut flex, &clock_text, clock_px);
    let sub = p.text_leaf(&mut flex, &sub_text, small_px);
    let center = flex.node(
        Container::column().width(center_w.max(p.txt.width(&sub_text, small_px) + 20.0 * s))
            .height(bar_h).gap(7.0 * s)
            .align(AlignItems::CENTER).justify(JustifyContent::CENTER),
        &[clock, sub],
    );
    let bar = flex.node(
        Container::row().align(AlignItems::STRETCH),
        &[teams[0].block, teams[0].score_box, center, teams[1].score_box, teams[1].block],
    );
    let lane = flex.absolute(
        Container::row().width(w).justify(JustifyContent::CENTER),
        0.0,
        14.0 * s,
        &[bar],
    );
    let screen = flex.node(Container::column().width(w).height(h), &[lane]);
    flex.compute(screen, w, h);

    for (side, nodes) in SIDES.iter().zip(&teams) {
        let block = flex.bounds(nodes.block);
        p.fill(out, block, PLATE);
        // Team accent along the bottom edge of the block.
        let rule = 3.0 * s;
        p.fill(out, [block[0], block[1] + block[3] - rule, block[2], rule], side.color);
        p.text(out, side.label, flex.bounds(nodes.name), name_px, INK);
        p.text(out, &nodes.count_text, flex.bounds(nodes.count), small_px, MUTED);
        for &(pip, alive) in &nodes.pips {
            p.fill(out, flex.bounds(pip), if alive { side.color } else { fade(FAINT, 0.6) });
        }
        let score_box = flex.bounds(nodes.score_box);
        let deep = fade(side.color, 0.55);
        let (from, to) = if side.mirrored { (fade(side.color, 0.92), deep) } else { (deep, fade(side.color, 0.92)) };
        p.ramp(out, score_box, from, to);
        let score_text = b.team_scores[if side.mirrored { 1 } else { 0 }].to_string();
        p.text_centered(out, &score_text, flex.bounds(nodes.score), score_px, INK);
    }
    let center_b = flex.bounds(center);
    p.fill(out, center_b, PLATE_DEEP);
    p.text_centered(out, &clock_text, flex.bounds(clock), clock_px, INK);
    p.text_centered(out, &sub_text, flex.bounds(sub), small_px, MUTED);
    if b.scanning {
        // Telemetry sweep in progress: a small live marker in the clock box.
        let dot = 5.0 * s;
        p.fill(out, [center_b[0] + center_b[2] - dot - 6.0 * s, center_b[1] + 6.0 * s, dot, dot], fade(INK, 0.7));
    }
    let bar_b = flex.bounds(bar);
    bar_b[1] + bar_b[3]
}

/// For JK2, authoritative CS_FLAGSTATUS decides whether a carried flag
/// actually belongs on a player's card. Never trust stale interpolated entity
/// powerups when the server says that flag was dropped. JKA uses its normal
/// observed flag powerups (there is no JK2 enhanced-status overlay).
fn card_flag(player: &BroadcastPlayerUi, ui: &UiSnapshot) -> Option<(i32, Option<u32>)> {
    if ui.jk2_mode {
        // Prefer CS_FLAGSTATUS when available; only a flag whose status has
        // not been received at all may fall back to live entity powerups.
        let confirmed = ui.jk2_flags.iter()
            .find(|flag| flag.status == 1 && flag.carrier_client == Some(player.client))
            .map(|flag| (flag.team, (flag.held_ms > 0).then_some(flag.held_ms)));
        confirmed.or_else(|| player.carrying_flag
            .filter(|team| !ui.jk2_flags.iter().any(|flag| flag.team == *team))
            .map(|team| (team, None)))
    } else {
        player.carrying_flag.map(|team| (team, None))
    }
}

fn missing_carrier_flags<'a>(
    ui: &'a UiSnapshot, team: i32, players: &[&BroadcastPlayerUi],
) -> Vec<&'a super::Jk2FlagUi> {
    ui.jk2_flags.iter().filter(|flag| {
        // Bucky MVSDK can have an opt-in neutral/yellow third flag, which
        // has no dedicated spectator team stack. Attach it to RED's header.
        (flag.team == team || (team == TEAM_RED && flag.team == 0))
            && (flag.status == 4 || (flag.status == 1
                && !players.iter().any(|p| Some(p.client) == flag.carrier_client)))
    }).collect()
}

/// Player cards: RED bottom-left, BLUE bottom-right (mirrored). `tops` is
/// the highest card edge per side, in `SIDES` order.
fn player_stacks(out: &mut Vec<UiVertex>, p: &Paint, b: &SpectatorBroadcastUi, tops: [f32; 2], bottom: f32) {
    let s = p.s;
    let (w, h) = (p.w as f32, p.h as f32);
    // Square avatar at the outer edge, then the 300-unit info body.
    let body_w = 300.0 * s;
    let top_h = 30.0 * s;
    let low_h = 20.0 * s;
    let card_h = top_h + low_h;
    let avatar_w = card_h;
    let card_w = avatar_w + body_w;
    let gap = 4.0 * s;
    let margin = 16.0 * s;
    let hp_w = 78.0 * s;
    let icon_w = 56.0 * s;
    let name_px = 16.0 * s;
    let hp_px = 19.0 * s;
    let small_px = 12.0 * s;
    let observed = b.observed.as_ref().map(|observed| observed.client);

    for (side, top) in SIDES.into_iter().zip(tops) {
        // Reserve a fixed flag-status header in JK2 CTF/CTY. That way a
        // pickup/drop changes only the icon/label, never the number or
        // placement of player cards (including during telemetry probes).
        let status_h = if p.ui.jk2_mode && matches!(b.game_type, 8 | 9) { 27.0 * s } else { 0.0 };
        let fit_rows = (((bottom - top - status_h) + gap) / (card_h + gap)).floor().max(0.0) as usize;
        let players: Vec<&BroadcastPlayerUi> = b.players.iter()
            .filter(|pl| pl.team == side.team)
            .take(fit_rows)
            .collect();
        if players.is_empty() {
            continue;
        }
        struct Card {
            card: NodeId,
            avatar: NodeId,
            top_row: NodeId,
            hp: NodeId,
            name: NodeId,
            name_text: String,
            icon: Option<(NodeId, usize)>,
            ammo: NodeId,
            ammo_text: String,
            force: NodeId,
            speed: NodeId,
            speed_text: String,
            kd: NodeId,
            kd_text: String,
        }
        let mut flex = Flex::new();
        let mut cards = Vec::new();
        for player in &players {
            let alive = player.alive != Some(false);
            let name_max = body_w - hp_w - icon_w - 28.0 * s;
            let base_name = if alive { player.name.clone() } else { strip_colors(&player.name) };
            let name_text = p.fit(&base_name, name_px, name_max);
            let hp = flex.leaf(hp_w, top_h);
            let name = p.text_leaf(&mut flex, &name_text, name_px);
            let spring = flex.spring();
            let weapon = has_weapon_icon(player.weapon).filter(|_| alive);
            let icon = flex.leaf(icon_w, 22.0 * s);
            let mut row: Vec<NodeId> = vec![hp, name, spring, icon];
            if side.mirrored {
                row.reverse();
            }
            let top_row = flex.node(Container::row().height(top_h).gap(9.0 * s), &row);

            let ammo_text = player.ammo.map_or_else(String::new, |ammo| format!("AMMO {}", ammo.max(0)));
            let kd_text = match (player.score, player.deaths) {
                (Some(score), Some(deaths)) => format!("{score} / {deaths}"),
                (Some(score), None) => score.to_string(),
                _ => "--".to_owned(),
            };
            let ammo = p.text_leaf(&mut flex, &ammo_text, small_px);
            let force = flex.leaf(52.0 * s, 4.0 * s);
            let speed_text = if alive {
                player.speed.map_or_else(|| "--".to_owned(), |speed| format!("{speed} ups"))
            } else { "--".to_owned() };
            // Fixed width prevents speed changes from moving neighbouring stats.
            let speed = flex.leaf(65.0 * s, low_h);
            let low_spring = flex.spring();
            let kd = p.text_leaf(&mut flex, &kd_text, small_px);
            let mut low: Vec<NodeId> = vec![ammo, force, speed, low_spring, kd];
            if side.mirrored {
                low.reverse();
            }
            let low_row = flex.node(
                Container::row().height(low_h).pad(10.0 * s, 0.0).gap(7.0 * s),
                &low,
            );
            let body = flex.node(Container::column().width(body_w).height(card_h), &[top_row, low_row]);
            let avatar = flex.leaf(avatar_w, card_h);
            let parts = if side.mirrored { [body, avatar] } else { [avatar, body] };
            let card = flex.node(Container::row().width(card_w).height(card_h), &parts);
            cards.push(Card {
                card, avatar, top_row, hp, name, name_text,
                icon: weapon.map(|weapon| (icon, weapon)),
                ammo, ammo_text, force, speed, speed_text, kd, kd_text,
            });
        }
        let stack_h = cards.len() as f32 * card_h + cards.len().saturating_sub(1) as f32 * gap;
        let x = if side.mirrored { w - margin - card_w } else { margin };
        let nodes: Vec<NodeId> = cards.iter().map(|card| card.card).collect();
        let stack = flex.absolute(Container::column().width(card_w).gap(gap), x, bottom - stack_h, &nodes);
        let screen = flex.node(Container::column().width(w).height(h), &[stack]);
        flex.compute(screen, w, h);

        // This strip is attached directly to the corresponding player stack,
        // not a second cg_enhancedFlagStatus overlay in the screen corner.
        // A dropped flag (or unknown carrier) has no player card to decorate.
        let missing = missing_carrier_flags(p.ui, side.team, &players);
        if !missing.is_empty() {
            let y = bottom - stack_h - status_h + 2.0 * s;
            let tile_w = card_w / missing.len() as f32;
            p.fill(out, [x, y, card_w, status_h - 4.0 * s], PLATE_DEEP);
            for (i, flag) in missing.iter().enumerate() {
                let tile_x = x + i as f32 * tile_w;
                let size = 22.0 * s;
                let icon_x = if side.mirrored { tile_x + tile_w - size - 5.0 * s }
                    else { tile_x + 5.0 * s };
                if let Some(image) = flag_status_icon(flag.team, flag.status) {
                    p.hud_image(out, image, [icon_x, y, size, size]);
                }
                let label = if flag.status == 4 { "DROPPED" } else { "CARRIER UNKNOWN" };
                let text_w = p.txt.width(label, small_px);
                let tx = if side.mirrored { icon_x - text_w - 6.0 * s }
                    else { icon_x + size + 6.0 * s };
                p.txt.draw(out, label, tx, y + 16.0 * s, small_px, MUTED);
            }
        }

        for (player, card) in players.iter().zip(&cards) {
            let alive = player.alive != Some(false);
            let is_observed = observed == Some(player.client);
            let vitals = player.vitals.as_ref();
            let stale = vitals.is_some_and(|v| v.age_ms > STALE_MS);
            // Exact value decides the low-health tint; the fill animates.
            let low = vitals.and_then(health_fraction).filter(|_| alive).is_some_and(|f| f <= 0.25);
            let mille = |v: u16| f32::from(v.min(1000)) / 1000.0;
            let health_bar = player.health_bar.filter(|_| alive).map(|(fill, trail)| (mille(fill), mille(trail)));
            let card_b = flex.bounds(card.card);
            p.fill(out, card_b, if !alive { PLATE_DEAD } else if is_observed { [0.09, 0.10, 0.13, 0.90] } else { PLATE });

            // Health fills the top row behind the name from the HP box side.
            let row_b = flex.bounds(card.top_row);
            let hp_b = flex.bounds(card.hp);
            if let Some((fraction, trail)) = health_bar {
                let tone = if low { HEALTH_LOW } else { side.color };
                let strength = if stale { 0.5 } else { 1.0 };
                let span = row_b[2] - hp_b[2];
                let fill_w = span * fraction;
                let trail_w = span * trail;
                let (fx, tx, from, to) = if side.mirrored {
                    (hp_b[0] - fill_w, hp_b[0] - trail_w, fade(tone, 0.22 * strength), fade(tone, 0.62 * strength))
                } else {
                    (hp_b[0] + hp_b[2], hp_b[0] + hp_b[2] + fill_w, fade(tone, 0.62 * strength), fade(tone, 0.22 * strength))
                };
                if trail_w > fill_w + 0.5 {
                    p.fill(out, [tx, row_b[1], trail_w - fill_w, row_b[3]], fade(TRAIL, 0.38 * strength));
                }
                p.ramp(out, [fx, row_b[1], fill_w, row_b[3]], from, to);
            }
            p.fill(out, hp_b, PLATE_DEEP);
            avatar(out, p, player, flex.bounds(card.avatar), side.color, alive);
            let accent = 3.0 * s;
            let edge_x = if side.mirrored { card_b[0] + card_b[2] - accent } else { card_b[0] };
            p.fill(out, [edge_x, card_b[1], accent, card_b[3]], if alive { side.color } else { FAINT });
            if is_observed {
                p.outline(out, card_b, (1.5 * s).max(1.0), fade(INK, 0.85));
            }

            // "100/25": health red, armor green (dimmed when stale).
            match (alive, vitals.and_then(|v| v.health)) {
                (false, _) => p.text_centered(out, "DEAD", hp_b, small_px, FAINT),
                (true, None) => p.text_centered(out, "?", hp_b, hp_px, MUTED),
                (true, Some(health)) => {
                    let tone = |color: [f32; 4]| if stale { fade(color, 0.55) } else { color };
                    // Fixed anchor: the "/" sits at a constant x; health is
                    // right-aligned to it and armor left-aligned after it, so
                    // changing digits grow outward instead of shifting both.
                    let health_text = health.max(0).to_string();
                    let armor_text = vitals.and_then(|v| v.armor)
                        .map(|armor| format!("/{}", armor.max(0)));
                    let slash_x = hp_b[0] + hp_b[2] * 0.52;
                    let baseline = p.txt.baseline_in(hp_b, hp_px);
                    p.txt.draw_right(out, &health_text, slash_x, baseline, hp_px,
                        tone(if low { HEALTH_LOW } else { HP_RED }));
                    if let Some(text) = armor_text.as_deref() {
                        p.txt.draw(out, text, slash_x, baseline, hp_px, tone(AR_GREEN));
                    }
                }
            }
            p.text(out, &card.name_text, flex.bounds(card.name), name_px, if alive { INK } else { FAINT });
            if let Some((icon, weapon)) = card.icon {
                p.weapon_icon(out, weapon, flex.bounds(icon), fade(INK, if stale { 0.55 } else { 0.85 }));
            }

            if !card.ammo_text.is_empty() && alive {
                p.text(out, &card.ammo_text, flex.bounds(card.ammo), small_px, MUTED);
            }
            let force_b = flex.bounds(card.force);
            if let Some((fraction, trail)) = player.force_bar.filter(|_| alive) {
                p.fill(out, force_b, fade(FAINT, 0.5));
                let fill_w = force_b[2] * mille(fraction);
                let trail_w = force_b[2] * mille(trail);
                let right = force_b[0] + force_b[2];
                let (fx, tx) = if side.mirrored {
                    (right - fill_w, right - trail_w)
                } else {
                    (force_b[0], force_b[0] + fill_w)
                };
                if trail_w > fill_w + 0.5 {
                    p.fill(out, [tx, force_b[1], trail_w - fill_w, force_b[3]], fade(TRAIL, if stale { 0.3 } else { 0.6 }));
                }
                p.fill(out, [fx, force_b[1], fill_w, force_b[3]], fade(FORCE, if stale { 0.5 } else { 0.95 }));
            }
            p.text_centered(out, &card.speed_text, flex.bounds(card.speed), small_px,
                if !alive { FAINT } else if player.speed_stale { fade(MUTED, 0.65) } else { INK });
            p.text(out, &card.kd_text, flex.bounds(card.kd), small_px, if alive { INK } else { FAINT });
        }
    }
}

/// The player's model icon in a square portrait (same atlas and missing-model
/// stand-in as the scoreboard), washed with the team colour at its foot.
fn avatar(out: &mut Vec<UiVertex>, p: &Paint, player: &BroadcastPlayerUi, b: Bounds, team: [f32; 4], alive: bool) {
    p.fill(out, b, PLATE_DEEP);
    let found = p.ui.avatar_atlas.as_ref().and_then(|atlas| {
        atlas.uv(&player.model).map(|uv| (uv, 1.0))
            .or_else(|| atlas.uv(&player.model_fallback).map(|uv| (uv, 0.45)))
    });
    if let Some(((uv0, uv1), strength)) = found {
        let alpha = strength * if alive { 1.0 } else { 0.3 };
        let inset = 2.0 * p.s;
        textured_rect_with_source(out, b[0] + inset, b[1] + inset, b[2] - inset * 2.0, b[3] - inset * 2.0,
            uv0, uv1, [1.0, 1.0, 1.0, alpha], AVATAR_SOURCE, p.w, p.h);
    }
    if alive {
        let wash = b[3] * 0.38;
        rect_gradient(out, b[0], b[1] + b[3] - wash, b[2], wash,
            fade(team, 0.0), fade(team, 0.0), fade(team, 0.42), fade(team, 0.42), p.w, p.h);
        if let Some((flag_team, held_ms)) = card_flag(player, p.ui) {
            // The scoreboard/MVSDK image is the actual flag being carried,
            // not a new glyph or the color of the carrier's own team.
            let size = 23.0 * p.s;
            p.flag_icon(out, flag_team, [b[0] + b[2] - size - 2.0 * p.s,
                b[1] + 2.0 * p.s, size, size]);
            if let Some(ms) = held_ms {
                let secs = ms / 1000;
                let time = format!("{}:{:02}", secs / 60, secs % 60);
                let strip = [b[0], b[1] + b[3] - 15.0 * p.s, b[2], 15.0 * p.s];
                p.fill(out, strip, PLATE_DEEP);
                p.text_centered(out, &time, strip, 11.0 * p.s, INK);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_colors_keeps_plain_carets() {
        assert_eq!(strip_colors("^1Bro^7die"), "Brodie");
        assert_eq!(strip_colors("a^ b"), "a^ b");
    }

    #[test]
    fn broadcast_uses_the_stock_ctf_scoreboard_flag_art() {
        let mut images = vec!["gfx/hud/mpi_rflag".to_owned()];
        append_broadcast_flag_images(&mut images);
        append_broadcast_flag_images(&mut images);
        assert_eq!(images, vec!["gfx/hud/mpi_rflag", "gfx/hud/mpi_bflag"]);
        assert_eq!(flag_carrier_icon(TEAM_RED), Some("gfx/hud/mpi_rflag"));
        assert_eq!(flag_carrier_icon(TEAM_BLUE), Some("gfx/hud/mpi_bflag"));
        assert_eq!(flag_status_icon(TEAM_RED, 1), Some("gfx/hud/mpi_rflag_x"));
        assert_eq!(flag_status_icon(TEAM_RED, 4), Some("gfx/hud/mpi_rflag"));
    }

    #[test]
    fn flag_status_lives_in_cards_and_discarded_powerups_cannot_override_drops() {
        let player = BroadcastPlayerUi {
            client: 7, team: TEAM_BLUE, name: "Runner".into(),
            score: None, deaths: None, weapon: None, ammo: None,
            speed: None, speed_stale: false, alive: Some(true), vitals: None,
            model: String::new(), model_fallback: String::new(),
            position: None, yaw: 0, position_stale: false,
            carrying_flag: Some(TEAM_RED), health_bar: None, force_bar: None,
        };
        let mut ui = UiSnapshot { jk2_mode: true, ..Default::default() };
        ui.jk2_flags.push(super::super::Jk2FlagUi {
            team: TEAM_RED, status: 4, carrier_client: None, carrier: None,
            carrier_team: None, health_armor: None, held_ms: 0,
        });
        // The entity's old flag powerup cannot light a card after a drop.
        assert_eq!(card_flag(&player, &ui), None);
        assert_eq!(missing_carrier_flags(&ui, TEAM_RED, &[&player]).len(), 1);

        // Confirmed pickup attaches both image and time to the correct card.
        ui.jk2_flags[0].status = 1;
        ui.jk2_flags[0].carrier_client = Some(7);
        ui.jk2_flags[0].held_ms = 91_000;
        assert_eq!(card_flag(&player, &ui), Some((TEAM_RED, Some(91_000))));
        assert!(missing_carrier_flags(&ui, TEAM_RED, &[&player]).is_empty());

        // If the carrier is unknown, never label a guessed player.
        ui.jk2_flags[0].carrier_client = None;
        assert_eq!(card_flag(&player, &ui), None);
        assert_eq!(missing_carrier_flags(&ui, TEAM_RED, &[&player]).len(), 1);

        // The optional third-team flag has no card stack of its own.
        ui.jk2_flags[0].team = 0;
        ui.jk2_flags[0].status = 4;
        assert_eq!(missing_carrier_flags(&ui, TEAM_RED, &[&player]).len(), 1);
        assert!(missing_carrier_flags(&ui, TEAM_BLUE, &[&player]).is_empty());

        // JKA still uses the native observed flag powerups and the same art.
        ui.jk2_mode = false;
        assert_eq!(card_flag(&player, &ui), Some((TEAM_RED, None)));
    }

    #[test]
    fn health_fraction_uses_max_health() {
        let vitals = BroadcastVitals { health: Some(125), max_health: Some(125), armor: None, force: None, age_ms: 0 };
        assert_eq!(health_fraction(&vitals), Some(1.0));
        let vitals = BroadcastVitals { max_health: None, health: Some(50), ..vitals };
        assert_eq!(health_fraction(&vitals), Some(0.5));
    }
}
