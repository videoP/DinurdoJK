//! Hold `/emote` to open a nonmodal, relative-mouse, two-tier emote wheel.
//! Unlike release-to-commit wheels, it mirrors `/dev`: left-click commits a
//! selection, release of the bound key/button only cancels. Right-click goes
//! back to categories. The outer tier never paginates or steals gameplay input.
use super::egui_dev_radial::{paint_annular_sector, paint_arc};
use super::egui_theme as theme;
use super::*;
use crate::net::mod_support::ServerMod;
use std::borrow::Cow;
use std::f32::consts::{FRAC_PI_2, PI};

const CATEGORY_INNER: f32 = 0.13;
const CATEGORY_OUTER: f32 = 0.48;
const EMOTE_INNER: f32 = 0.505;
const SECTOR_GAP: f32 = 0.016;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum EmoteCategory {
    Taunts,
    Standard,
    Library,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Action {
    Category(EmoteCategory),
    Command(&'static str),
    /// A starred emote from the installed library, by catalog ID.
    Emote(String),
    Disabled,
}

#[derive(Clone, Debug)]
struct Entry {
    label: Cow<'static, str>,
    sub: Cow<'static, str>,
    action: Action,
}

impl Entry {
    const fn new(label: &'static str, sub: &'static str, action: Action) -> Self {
        Self { label: Cow::Borrowed(label), sub: Cow::Borrowed(sub), action }
    }
    const fn cmd(label: &'static str, command: &'static str) -> Self {
        Self::new(label, command, Action::Command(command))
    }
    fn enabled(&self) -> bool {
        self.action != Action::Disabled
    }
}

// Real, separately documented mod commands: JA+ emote documentation and
// jaPRO's g_cmds / documented command table (the latter is also enumerated
// in this client as console::JAPRO_COMMANDS). These are server commands, not
// client-invented animation IDs. Never mix jaPRO and JA+ entries on a server.
// JA+ list supplied by the user. Keep the display name separate from the
// server command: JA+ calls amFlip "AmIgnition", for example.
const JAPLUS_EMOTES: &[Entry] = &[
    Entry::cmd("AM TEASE", "amATease"),
    Entry::cmd("AM BEG", "amBeg"),
    Entry::cmd("COME ON", "amComeOn"),
    Entry::cmd("BREAKDANCE 1", "amBreakDance1"),
    Entry::cmd("BREAKDANCE 2", "amBreakDance2"),
    Entry::cmd("FINISHING HIM", "amFinishingHim"),
    Entry::cmd("IGNITION", "amFlip"),
    Entry::cmd("HELLO", "amHello"),
    Entry::cmd("HILT THROW 1", "amHiltThrow1"),
    Entry::cmd("HILT THROW 2", "amHiltThrow2"),
    Entry::cmd("HIPS", "amHips"),
    Entry::cmd("HUG", "amHug"),
    Entry::cmd("KISS", "amKiss"),
    Entry::cmd("KNEEL", "amKneel"),
    Entry::cmd("NEO", "amNeo"),
    Entry::cmd("NOD", "amNod"),
    Entry::cmd("NOISY", "amNoisy"),
    Entry::cmd("POWER", "amPower"),
    Entry::cmd("SHAKE", "amShake"),
    Entry::cmd("SIT 1", "amSit1"),
    Entry::cmd("SIT 2", "amSit2"),
    Entry::cmd("SIT 3", "amSit3"),
    Entry::cmd("SIT 4", "amSit4"),
    Entry::cmd("SURRENDER", "amSurrender"),
    Entry::cmd("VICTORY", "amVictory"),
    Entry::cmd("WAIT", "amWait"),
    Entry::cmd("WON", "amWon"),
    Entry::cmd("MEDITATE", "Meditate"),
    Entry::cmd("SLEEP", "Sleep"),
];

// Actual emote handlers registered by TaystJK (codemp/game/g_cmds.c, `//EMOTE`).
// No client-only tab-completion names or nonexistent dance aliases.
const JAPRO_EMOTES: &[Entry] = &[
    Entry::cmd("BEG", "ambeg"),
    Entry::cmd("BEG 2", "ambeg2"),
    Entry::cmd("BERNIE", "ambernie"),
    Entry::cmd("BREAKDANCE 1", "ambreakdance"),
    Entry::cmd("BREAKDANCE 2", "ambreakdance2"),
    Entry::cmd("BREAKDANCE 3", "ambreakdance3"),
    Entry::cmd("BREAKDANCE 4", "ambreakdance4"),
    Entry::cmd("CHEER", "amcheer"),
    Entry::cmd("COWER", "amcower"),
    Entry::cmd("DANCE", "amdance"),
    Entry::cmd("FLIP", "amflip"),
    Entry::cmd("HUG", "amhug"),
    Entry::cmd("NOISY", "amnoisy"),
    Entry::cmd("POINT", "ampoint"),
    Entry::cmd("RAGE", "amrage"),
    Entry::cmd("RUN", "amrun"),
    Entry::cmd("SIGNAL 1", "amsignal"),
    Entry::cmd("SIGNAL 2", "amsignal2"),
    Entry::cmd("SIGNAL 3", "amsignal3"),
    Entry::cmd("SIGNAL 4", "amsignal4"),
    Entry::cmd("SIT 1", "amsit"),
    Entry::cmd("SIT 2", "amsit2"),
    Entry::cmd("SIT 3", "amsit3"),
    Entry::cmd("SIT 4", "amsit4"),
    Entry::cmd("SIT 5", "amsit5"),
    Entry::cmd("SLAP", "amslap"),
    Entry::cmd("SLEEP", "amsleep"),
    Entry::cmd("SMACK", "amsmack"),
    Entry::cmd("SURRENDER", "amsurrender"),
    Entry::cmd("TAUNT", "amtaunt"),
    Entry::cmd("TAUNT 2", "amtaunt2"),
    Entry::cmd("VICTORY", "amvictory"),
];

fn standard_catalog(server_mod: ServerMod) -> &'static [Entry] {
    match server_mod {
        ServerMod::Japlus => JAPLUS_EMOTES,
        ServerMod::Japro => JAPRO_EMOTES,
        _ => &[],
    }
}

fn categories() -> [Entry; 4] {
    [
        Entry::new("TAUNTS", "SABER + GESTURES", Action::Category(EmoteCategory::Taunts)),
        Entry::new("STANDARD", "SERVER MOD", Action::Category(EmoteCategory::Standard)),
        Entry::new("LIBRARY", "STARRED EMOTES", Action::Category(EmoteCategory::Library)),
        Entry::new("FAVORITES", "COMING SOON", Action::Disabled),
    ]
}

/// Every emote in one uninterrupted ring: no Back, Prev, Next, or page slots.
/// `library` is the user's starred library emotes (see `library_entries`).
fn entries<'a>(category: EmoteCategory, server_mod: ServerMod, library: &'a [Entry]) -> &'a [Entry] {
    match category {
        EmoteCategory::Taunts => const { &[
            Entry::cmd("TAUNT", "taunt"),
            Entry::cmd("FLOURISH", "flourish"),
            Entry::cmd("GLOAT", "gloat"),
            Entry::cmd("BOW", "bow"),
            Entry::cmd("MEDITATE", "meditate"),
        ] },
        EmoteCategory::Standard => standard_catalog(server_mod),
        EmoteCategory::Library => library,
    }
}

/// Starred emotes in Profile order, or one disabled entry saying why the
/// tier is empty. Never the whole library: the ring does not paginate.
fn library_entries(favorites: Result<Vec<crate::emotes::Emote>, &'static str>) -> Vec<Entry> {
    match favorites {
        Ok(emotes) => emotes
            .into_iter()
            .map(|emote| Entry {
                label: Cow::Owned(crate::emotes::display_title(&emote).to_uppercase()),
                sub: Cow::Owned(emote.id.clone()),
                action: Action::Emote(emote.id),
            })
            .collect(),
        Err(reason) => vec![Entry::new(reason, "PROFILE > EMOTES", Action::Disabled)],
    }
}

/// Top sector is index 0; the virtual stick has no center or rim dead zone,
/// matching `/dev`'s angle-only selection.
fn selected_sector(offset: [f32; 2], count: usize) -> usize {
    if offset == [0.0, 0.0] || count == 0 {
        return 0;
    }
    let sweep = 2.0 * PI / count as f32;
    let start = -FRAC_PI_2 - sweep * 0.5;
    ((offset[1].atan2(offset[0]) - start).rem_euclid(2.0 * PI) / sweep).floor() as usize % count
}

/// Paint a radial label along its sector centerline. Narrow wedges can show
/// arbitrarily many complete emote names because the text uses the *radial*
/// length of the annulus, rather than trying to fit across its short arc.
fn paint_radial_label(
    painter: &egui::Painter,
    center: egui::Pos2,
    angle: f32,
    radius: f32,
    label: &str,
    font_size: f32,
    color: egui::Color32,
) {
    let font = egui::FontId::proportional(font_size);
    let galley = painter.layout_no_wrap(label.to_owned(), font, color);
    // Rotate characters clockwise along the outward spoke. On the left half
    // flip the text so it is read upright rather than upside down.
    let rotation = if angle.cos() < 0.0 { angle + PI } else { angle };
    let origin = center + egui::vec2(angle.cos(), angle.sin()) * radius;
    let (sin, cos) = rotation.sin_cos();
    let local = egui::vec2(-galley.size().x * 0.5, -galley.size().y * 0.5);
    let top_left = origin + egui::vec2(local.x * cos - local.y * sin,
                                       local.x * sin + local.y * cos);
    painter.add(egui::Shape::Text(
        egui::epaint::TextShape::new(top_left, galley, color).with_angle(rotation),
    ));
}

/// Text must fit within the radial band, never spill over the inner category
/// ring or beyond the outer radius. No entry is removed due to length/count.
fn emote_font_size(painter: &egui::Painter, label: &str, radial_space: f32) -> f32 {
    let mut size = 12.0;
    while size > 6.0 {
        let text = painter.layout_no_wrap(label.to_owned(), egui::FontId::proportional(size), egui::Color32::WHITE);
        if text.size().x <= radial_space { break; }
        size -= 0.5;
    }
    size
}

impl App {
    pub(in crate::app) fn emote_radial_back(&mut self) {
        if self.emote_radial_category.is_some() {
            self.emote_radial_category = None;
            self.dev_radial_offset = [0.0, -0.28];
            self.egui_repaint_requested = true;
        } else {
            self.close_emote_radial();
        }
    }

    pub(in crate::app) fn activate_emote_radial_selection(&mut self) {
        if self.overlay != OverlayMode::EmoteRadial {
            return;
        }
        let mod_id = self.ui_mod().unwrap_or(self.connected_server_mod());
        let selected = if let Some(category) = self.emote_radial_category {
            let library = self.radial_library_entries(category);
            let choices = entries(category, mod_id, &library);
            if choices.is_empty() { return; }
            choices[selected_sector(self.dev_radial_offset, choices.len())].clone()
        } else {
            let choices = categories();
            choices[selected_sector(self.dev_radial_offset, choices.len())].clone()
        };
        match selected.action {
            Action::Category(category) => {
                self.emote_radial_category = Some(category);
                self.dev_radial_offset = [0.0, -0.28];
                self.egui_repaint_requested = true;
                if category == EmoteCategory::Library {
                    self.warm_favorite_emote_audio();
                }
            }
            Action::Command(command) => {
                self.close_emote_radial();
                self.append_console_commands(vec![command.to_owned()]);
            }
            Action::Emote(id) => {
                self.close_emote_radial();
                self.play_favorite_emote(&id);
            }
            Action::Disabled => {}
        }
    }

    /// Only the LIBRARY tier reads the library; other tiers are static.
    fn radial_library_entries(&self, category: EmoteCategory) -> Vec<Entry> {
        if category == EmoteCategory::Library {
            library_entries(self.wheel_favorite_emotes())
        } else {
            Vec::new()
        }
    }

    pub(super) fn egui_emote_radial(&mut self, root: &mut egui::Ui) {
        let rect = root.max_rect();
        let center = rect.center();
        // Keep the outer tier on screen at low resolution. At 1080p it has
        // enough radial space for long names like FINISHING HIM in 29 wedges.
        let outer = ((rect.width() - 74.0).min(rect.height() - 88.0) * 0.5)
            .clamp(65.0, 470.0);
        let painter = root.painter();
        let server_mod = self.ui_mod().unwrap_or(self.connected_server_mod());
        let chosen_category = self.emote_radial_category;
        let library = chosen_category.map_or_else(Vec::new, |category| self.radial_library_entries(category));
        let catalog = chosen_category.map(|category| entries(category, server_mod, &library));
        let selected_outer = catalog.and_then(|items| {
            (!items.is_empty()).then(|| selected_sector(self.dev_radial_offset, items.len()))
        });
        let selected_inner = if chosen_category.is_none() {
            Some(selected_sector(self.dev_radial_offset, 4))
        } else { None };

        painter.rect_filled(rect, egui::CornerRadius::ZERO, egui::Color32::from_black_alpha(92));
        painter.circle_filled(center, outer + 11.0, egui::Color32::from_black_alpha(68));

        // Paint the entire outer catalog, with radial text so every narrow
        // slice has an individually visible full label even with 30+ emotes.
        if let Some(items) = catalog {
            if !items.is_empty() {
                let step = 2.0 * PI / items.len() as f32;
                let inner = outer * EMOTE_INNER;
                let width = outer - inner;
                // Aim the label toward the middle of the band, leaving room
                // around both ends to avoid touching the inner ring.
                let label_radius = inner + width * 0.5;
                for (i, choice) in items.iter().enumerate() {
                    let angle = -FRAC_PI_2 + i as f32 * step;
                    let visual_gap = SECTOR_GAP.min(step * 0.16);
                    let start = angle - step * 0.5 + visual_gap;
                    let end = angle + step * 0.5 - visual_gap;
                    let highlighted = selected_outer == Some(i) && choice.enabled();
                    let fill = if highlighted {
                        egui::Color32::from_rgba_unmultiplied(38, 94, 151, 242)
                    } else if choice.enabled() {
                        egui::Color32::from_rgba_unmultiplied(17, 25, 37, 235)
                    } else {
                        egui::Color32::from_rgba_unmultiplied(24, 30, 39, 219)
                    };
                    paint_annular_sector(painter, center, inner, outer, start, end, fill);
                    paint_arc(painter, center, outer, start, end,
                        egui::Stroke::new(1.5, if highlighted { theme::ACCENT } else { theme::LINE_STRONG }));
                    paint_arc(painter, center, inner, start, end, egui::Stroke::new(1.0, theme::LINE));
                    let color = if highlighted { egui::Color32::WHITE }
                        else if choice.enabled() { theme::TEXT }
                        else { theme::TEXT_DISABLED };
                    let max_length = width - 18.0;
                    let size = emote_font_size(painter, &choice.label, max_length);
                    paint_radial_label(painter, center, angle, label_radius, &choice.label, size, color);
                }
            }
        }

        // Root categories never disappear. When a submenu is active, the whole
        // category ring is visibly dimmed and cannot accidentally activate.
        let inner_choices = categories();
        let step = 2.0 * PI / inner_choices.len() as f32;
        let ring_inner = outer * CATEGORY_INNER;
        let ring_outer = outer * CATEGORY_OUTER;
        for (i, choice) in inner_choices.iter().enumerate() {
            let angle = -FRAC_PI_2 + i as f32 * step;
            let start = angle - step * 0.5 + SECTOR_GAP;
            let end = angle + step * 0.5 - SECTOR_GAP;
            let highlighted = selected_inner == Some(i) && choice.enabled();
            let dimmed = chosen_category.is_some();
            let fill = if highlighted {
                egui::Color32::from_rgba_unmultiplied(38, 94, 151, 240)
            } else if dimmed {
                egui::Color32::from_rgba_unmultiplied(13, 20, 28, 195)
            } else if !choice.enabled() {
                egui::Color32::from_rgba_unmultiplied(24, 30, 39, 219)
            } else {
                egui::Color32::from_rgba_unmultiplied(17, 25, 37, 234)
            };
            paint_annular_sector(painter, center, ring_inner, ring_outer, start, end, fill);
            paint_arc(painter, center, ring_outer, start, end,
                egui::Stroke::new(1.3, if highlighted { theme::ACCENT } else { theme::LINE }));
            let fg = if highlighted { egui::Color32::WHITE }
                else if dimmed || !choice.enabled() { theme::TEXT_DISABLED }
                else { theme::TEXT };
            let anchor = center + egui::vec2(angle.cos(), angle.sin()) * (ring_inner + ring_outer) * 0.5;
            painter.text(anchor, egui::Align2::CENTER_CENTER, &*choice.label,
                egui::FontId::proportional(if outer < 220.0 { 9.0 } else { 11.0 }), fg);
        }

        painter.circle_filled(center, ring_inner - 3.0, theme::CHROME);
        painter.circle_stroke(center, ring_inner - 2.0, egui::Stroke::new(1.0, theme::LINE_STRONG));
        painter.text(center, egui::Align2::CENTER_CENTER, "EMOTE",
            egui::FontId::proportional(if outer > 300.0 { 15.0 } else { 10.0 }), theme::TEXT);

        // Full selected command below the rings assists reading tiny slices.
        let footer = if let (Some(_category), Some(items), Some(index)) =
            (chosen_category, catalog, selected_outer)
        {
            let choice = &items[index];
            match choice.action {
                Action::Command(_) => format!("{}  ·  /{}", choice.label, choice.sub),
                _ => format!("{}  ·  {}", choice.label, choice.sub),
            }
        } else if chosen_category.is_some() {
            "NO SERVER EMOTES AVAILABLE".to_owned()
        } else {
            "LEFT CLICK CATEGORY".to_owned()
        };
        painter.text(center + egui::vec2(0.0, outer + 17.0), egui::Align2::CENTER_CENTER,
            footer, egui::FontId::proportional(12.0), theme::TEXT);
        painter.text(center + egui::vec2(0.0, outer + 33.0), egui::Align2::CENTER_CENTER,
            if chosen_category.is_some() { "LEFT CLICK SELECT  ·  RIGHT CLICK BACK  ·  RELEASE CANCELS" }
            else { "LEFT CLICK OPEN  ·  RIGHT CLICK CLOSE  ·  RELEASE CANCELS" },
            egui::FontId::proportional(9.0), theme::TEXT_DIM);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn no_navigation_sectors_and_complete_standard_catalogs() {
        assert_eq!(categories().len(), 4);
        assert_eq!(categories()[3].action, Action::Disabled);
        assert_eq!(standard_catalog(ServerMod::Japlus).len(), 29);
        assert_eq!(standard_catalog(ServerMod::Japro).len(), 32);
        assert!(standard_catalog(ServerMod::Base).is_empty());
        for catalog in [JAPLUS_EMOTES, JAPRO_EMOTES] {
            let mut seen = HashSet::new();
            for entry in catalog {
                assert!(seen.insert(entry.sub.to_ascii_lowercase()), "duplicate emote {}", entry.sub);
                assert!(matches!(entry.action, Action::Command(_)));
            }
        }
        for command in ["amBreakDance1", "amBreakDance2", "amFlip", "amSit1", "Sleep"] {
            assert!(JAPLUS_EMOTES.iter().any(|entry| entry.sub.eq_ignore_ascii_case(command)));
        }
        for command in ["ambreakdance3", "ambreakdance4", "amsignal4", "amsit5", "amrun"] {
            assert!(JAPRO_EMOTES.iter().any(|entry| entry.sub == command));
        }
        // On selection the user sees every Standard entry, not a single page.
        assert_eq!(entries(EmoteCategory::Standard, ServerMod::Japro, &[]).len(), JAPRO_EMOTES.len());
        assert_eq!(entries(EmoteCategory::Standard, ServerMod::Japlus, &[]).len(), JAPLUS_EMOTES.len());
    }

    #[test]
    fn library_tier_lists_starred_emotes_or_says_why_it_is_empty() {
        let empty = library_entries(Err("STAR EMOTES IN PROFILE"));
        assert_eq!(empty.len(), 1);
        assert!(!empty[0].enabled());
        let emotes = jka_emotes::manifest::parse_manifest(&serde_json::json!({"emotes":[
            {"id":"EID_Floss","name":"Floss","gla":"models/b.gla","first_frame":0,"end_frame":30,"fps":30}]})).unwrap();
        let tier = library_entries(Ok(emotes));
        assert_eq!(tier[0].label, "FLOSS");
        assert_eq!(tier[0].action, Action::Emote("EID_Floss".into()));
        assert_eq!(entries(EmoteCategory::Library, ServerMod::Base, &tier).len(), 1);
    }

    #[test]
    fn sectors_are_stable_with_large_catalogs() {
        assert_eq!(selected_sector([0.0, -0.28], 4), 0);
        assert_eq!(selected_sector([1.0, 0.0], 4), 1);
        assert_eq!(selected_sector([0.0, 1.0], 4), 2);
        assert_eq!(selected_sector([-1.0, 0.0], 4), 3);
        for n in [5, 29, 32] {
            for i in 0..n {
                let angle = -FRAC_PI_2 + i as f32 * 2.0 * PI / n as f32;
                assert_eq!(selected_sector([angle.cos(), angle.sin()], n), i);
            }
        }
    }
}
