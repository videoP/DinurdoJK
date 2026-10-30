//! Profile -> Force. The allocation rules are stock JKA (`ProfileForceConfig`);
//! this is only how they are presented: two alignment cards with their
//! sabers, the point budget drawn as a blade that grows as points are spent,
//! and three kyber crystals per power for its level. Rows and cards are laid
//! out with Taffy (`ui::flex`).
use super::holo;
use super::{
    theme, App, ProfileForceConfig, FORCE_COSTS, FORCE_DARK_ORDER, FORCE_LIGHT_ORDER,
    FORCE_MASTERY_NAMES, FORCE_NAMES, FORCE_NEUTRAL_ORDER,
};
use crate::ui::flex::{Container, Flex};
use egui::{pos2, vec2, Color32, Rect, Sense};
use std::time::Instant;
use taffy::prelude::{AlignItems, JustifyContent};

const LIGHT: Color32 = Color32::from_rgb(0x5C, 0x9E, 0xFF);
const DARK: Color32 = Color32::from_rgb(0xFF, 0x4A, 0x3C);
/// Neutral powers glow in the palette's Celadon, a Jedi-green crystal.
const NEUTRAL: Color32 = theme::ACCENT;

fn to_rect(origin: egui::Pos2, b: [f32; 4]) -> Rect {
    Rect::from_min_size(pos2(origin.x + b[0], origin.y + b[1]), vec2(b[2], b[3]))
}

/// Why a power cannot be changed, if it cannot.
fn lock_reason(power: usize, disabled: u32, gametype: i32, force: &ProfileForceConfig) -> Option<&'static str> {
    if disabled & (1u32 << power) != 0 && !matches!(power, 1 | 15 | 16) {
        Some("Disabled by this server (g_forcePowerDisable).")
    } else if gametype < 6 && matches!(power, 11 | 12) {
        Some("Team powers only work in team game types.")
    } else if matches!(power, 16 | 17) && force.powers[15] == 0 {
        Some("Needs Saber Attack first.")
    } else {
        None
    }
}

impl App {
    pub(in crate::app::egui_menu) fn egui_profile_force(&mut self, ui: &mut egui::Ui) {
        let (max_rank, disabled, gametype, free_saber, forced_side) = self.profile_server_force_limits();
        let mut force = ProfileForceConfig::parse(&self.network.forcepowers);
        force.rank = max_rank;
        if let Some(side) = forced_side {
            force.side = side;
        }
        force.normalize(max_rank, disabled, gametype, free_saber);
        let before = force.serialize();

        self.force_alignment_cards(ui, &mut force, forced_side);
        ui.add_space(14.0);
        force.normalize(max_rank, disabled, gametype, free_saber);
        Self::force_budget_blade(ui, &force, max_rank, free_saber);
        ui.add_space(10.0);

        let side_color = if force.side == 2 { DARK } else { LIGHT };
        let aligned: &[usize] = if force.side == 2 { &FORCE_DARK_ORDER } else { &FORCE_LIGHT_ORDER };
        let mut clicked_power = None;
        for (title, powers, color) in [
            ("CORE POWERS", &FORCE_NEUTRAL_ORDER[..], NEUTRAL),
            (if force.side == 2 { "DARK SIDE POWERS" } else { "LIGHT SIDE POWERS" }, aligned, side_color),
        ] {
            theme::section(ui, title, "");
            for &power in powers {
                let min_level = if power == 1 || (free_saber && matches!(power, 15 | 16)) { 1 } else { 0 };
                let lock = lock_reason(power, disabled, gametype, &force);
                if let Some(level) = Self::force_power_row(ui, &force, power, color, min_level, lock, free_saber) {
                    if force.powers[power] != level {
                        force.powers[power] = level;
                        clicked_power = Some(power);
                    }
                }
            }
        }

        if let Some(power) = clicked_power {
            // Delay the animation until allocation clicks settle, so repeated
            // clicks restart a single preview.
            self.profile_force_preview_pending = Some((power, Instant::now()));
        }
        force.normalize(max_rank, disabled, gametype, free_saber);
        let after = force.serialize();
        if after != before {
            if let Err(error) = self.profile_set_cvar("forcepowers", &after) {
                self.console_status = error;
            } else {
                self.profile_force_pending = true;
            }
        }
    }

    /// Light and Dark as two cards, each with its saber drawn across it.
    fn force_alignment_cards(&self, ui: &mut egui::Ui, force: &mut ProfileForceConfig, forced_side: Option<u8>) {
        let width = ui.available_width();
        let mut flex = Flex::new();
        let light = flex.spring();
        let dark = flex.spring();
        // Stretch, so both cards take the row's full height.
        let row = flex.node(Container::row().width(width).height(92.0).gap(12.0).align(AlignItems::STRETCH), &[light, dark]);
        flex.compute(row, width, 92.0);
        let (area, _) = ui.allocate_exact_size(vec2(width, 92.0), Sense::hover());
        for (node, side, title, order, color) in [
            (light, 1u8, "LIGHT SIDE", "JEDI", LIGHT),
            (dark, 2u8, "DARK SIDE", "SITH", DARK),
        ] {
            let rect = to_rect(area.min, flex.bounds(node));
            let response = ui.interact(rect, ui.id().with(("force_side", side)), Sense::click());
            let selected = force.side == side;
            let locked = forced_side.is_some_and(|forced| forced != side);
            let hovered = response.hovered() && !locked;
            let painter = ui.painter();
            let fill = if selected {
                Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 26)
            } else if hovered {
                Color32::from_rgba_unmultiplied(0xF0, 0xF2, 0xEF, 10)
            } else {
                Color32::from_rgba_unmultiplied(0x10, 0x0E, 0x0C, 120)
            };
            painter.rect_filled(rect, 10.0, fill);
            let edge = if selected { color } else { theme::LINE };
            painter.rect_stroke(rect, 10.0, egui::Stroke::new(if selected { 1.5 } else { 1.0 }, edge), egui::StrokeKind::Inside);
            // The saber: lit when chosen, a bare hilt otherwise.
            let y = rect.top() + 30.0;
            let lit = if selected { 1.0 } else if hovered { 0.45 } else { 0.0 };
            holo::saber(ui.painter(), pos2(rect.left() + 22.0, y), pos2(rect.right() - 22.0, y), 3.0, color, lit);
            let cap = 12.0;
            let text_color = if selected { color } else if locked { theme::TEXT_DISABLED } else { theme::TEXT_DIM };
            holo::neon_text(ui.painter(), pos2(rect.left() + 22.0, rect.bottom() - 22.0), title, cap, text_color, if selected { 1.0 } else { 0.3 });
            ui.painter().text(
                pos2(rect.right() - 22.0, rect.bottom() - 26.0),
                egui::Align2::RIGHT_CENTER,
                order,
                egui::FontId::proportional(12.0),
                if selected { theme::TEXT } else { theme::TEXT_FAINT },
            );
            let response = if locked {
                response.on_hover_text("Fixed by g_forceBasedTeams for your current team.")
            } else {
                response
            };
            if hovered {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.clicked() && !locked {
                force.side = side;
            }
        }
    }

    /// Rank title, points, and a saber whose blade length is the share of
    /// the server's Force budget already spent.
    fn force_budget_blade(ui: &mut egui::Ui, force: &ProfileForceConfig, max_rank: u8, free_saber: bool) {
        let used = force.used(free_saber);
        let budget = force.budget().max(1);
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(width, 54.0), Sense::hover());
        let painter = ui.painter();
        let rank = FORCE_MASTERY_NAMES[max_rank.min(7) as usize].to_ascii_uppercase();
        holo::neon_text(painter, pos2(rect.left() + 2.0, rect.top() + 16.0), &rank, 13.0, theme::TEXT, 0.7);
        painter.text(
            pos2(rect.right(), rect.top() + 9.0),
            egui::Align2::RIGHT_CENTER,
            format!("{used} / {budget} POINTS"),
            egui::FontId::proportional(13.0),
            if used >= budget { theme::WARNING } else { theme::TEXT_DIM },
        );
        let side = if force.side == 2 { DARK } else { LIGHT };
        let y = rect.bottom() - 12.0;
        let hilt = pos2(rect.left() + 2.0, y);
        let full = rect.right() - 2.0;
        // Track the blade would fill, then the blade itself.
        painter.line_segment([pos2(hilt.x + 60.0, y), pos2(full, y)], egui::Stroke::new(3.0, Color32::from_rgba_unmultiplied(0x5C, 0x53, 0x46, 0x70)));
        let fraction = (used as f32 / budget as f32).clamp(0.0, 1.0);
        let tip = hilt.x + 60.0 + (full - hilt.x - 60.0) * fraction;
        let hilt_end = pos2(hilt.x + 58.0, y);
        holo::saber(painter, hilt, pos2(hilt.x + 58.0 / 0.24, y), 3.0, side, 0.0);
        if fraction > 0.0 {
            holo::blade(painter, hilt_end, pos2(tip, y), 3.0, side, 1.0);
        }
    }

    /// One power: name, the next level's cost, and three kyber crystals.
    /// Returns the level the player clicked.
    fn force_power_row(
        ui: &mut egui::Ui,
        force: &ProfileForceConfig,
        power: usize,
        color: Color32,
        min_level: u8,
        lock: Option<&'static str>,
        free_saber: bool,
    ) -> Option<u8> {
        const CRYSTAL: f32 = 16.0;
        let width = ui.available_width();
        let mut flex = Flex::new();
        let name = flex.spring();
        let cost = flex.leaf(92.0, 34.0);
        let crystals: Vec<_> = (0..3).map(|_| flex.leaf(28.0, 34.0)).collect();
        let mut children = vec![name, cost];
        children.extend(&crystals);
        let row = flex.node(
            Container::row().width(width).height(34.0).pad(10.0, 0.0).align(AlignItems::CENTER).justify(JustifyContent::FLEX_START),
            &children,
        );
        flex.compute(row, width, 34.0);
        let (area, row_response) = ui.allocate_exact_size(vec2(width, 34.0), Sense::hover());
        let painter = ui.painter().clone();
        if row_response.hovered() && lock.is_none() {
            painter.rect_filled(area, 8.0, Color32::from_rgba_unmultiplied(0xF0, 0xF2, 0xEF, 9));
        }
        let level = force.powers[power];
        let name_rect = to_rect(area.min, flex.bounds(name));
        painter.text(
            name_rect.left_center(),
            egui::Align2::LEFT_CENTER,
            FORCE_NAMES[power],
            egui::FontId::proportional(14.5),
            if lock.is_some() { theme::TEXT_DISABLED } else { theme::TEXT },
        );
        let cost_rect = to_rect(area.min, flex.bounds(cost));
        let cost_text = match lock {
            Some(_) => "LOCKED".to_owned(),
            None if level >= 3 => "MASTERED".to_owned(),
            None => format!("NEXT  {}", FORCE_COSTS[power][usize::from(level) + 1]),
        };
        painter.text(
            cost_rect.right_center() - vec2(10.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            cost_text,
            egui::FontId::proportional(11.5),
            if lock.is_some() { theme::TEXT_DISABLED } else { theme::TEXT_FAINT },
        );
        if let Some(reason) = lock {
            row_response.on_hover_text(reason);
        }
        let mut chosen = None;
        for (index, &node) in crystals.iter().enumerate() {
            let target = index as u8 + 1;
            let rect = to_rect(area.min, flex.bounds(node));
            let response = ui.interact(rect, ui.id().with(("kyber", power, index)), Sense::click());
            let lit = level >= target;
            // Clicking the top lit crystal steps back down a level.
            let next = if lit && level == target { target - 1 } else { target };
            let next = next.max(min_level);
            let mut trial = *force;
            trial.powers[power] = next;
            let affordable = trial.used(free_saber) <= trial.budget();
            let usable = lock.is_none() && (affordable || next < level);
            let hovered = response.hovered() && usable;
            let tint = if lock.is_some() { theme::TEXT_DISABLED } else if usable { color } else { theme::DANGER };
            holo::kyber(&painter, rect.center(), CRYSTAL, tint, lit, response.hovered() && lock.is_none());
            if hovered {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let response = if lock.is_none() && !usable {
                response.on_hover_text("Not enough Force points.")
            } else {
                response.on_hover_text(format!("Level {target}"))
            };
            if response.clicked() && usable && next != level {
                chosen = Some(next);
            }
        }
        ui.add_space(2.0);
        chosen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saber_powers_need_saber_attack_and_team_powers_need_teams() {
        let mut force = ProfileForceConfig { rank: 7, side: 1, powers: [0; 18] };
        assert!(lock_reason(16, 0, 0, &force).is_some());
        force.powers[15] = 1;
        assert!(lock_reason(16, 0, 0, &force).is_none());
        assert!(lock_reason(11, 0, 0, &force).is_some());
        assert!(lock_reason(11, 0, 6, &force).is_none());
        assert!(lock_reason(3, 1 << 3, 6, &force).is_some());
    }
}
