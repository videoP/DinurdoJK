//! Hold `/emote` to open a nonmodal, relative-mouse emote wheel with a categorized library.
//! Unlike release-to-commit wheels, it mirrors `/dev`: left-click commits a
//! selection, release of the bound key/button only cancels. Right-click goes
//! back to categories. The outer tier never paginates or steals gameplay input.
use super::egui_dev_radial::{paint_annular_sector, paint_arc};
use super::egui_theme as theme;
use super::*;
use crate::net::mod_support::ServerMod;
use std::borrow::Cow;
use std::f32::consts::{FRAC_PI_2, PI};

const CATEGORY_INNER: f32 = 0.09;
const CATEGORY_OUTER: f32 = 0.29;
const EMOTE_INNER: f32 = 0.315;
const LIBRARY_CATEGORY_OUTER: f32 = 0.55;
const LIBRARY_EMOTE_INNER: f32 = 0.575;
const SECTOR_GAP: f32 = 0.016;
// A wheel with hundreds of individually clickable wedges cannot be used.
// The library is grouped into at most 12 alphabetic sections, then into
// pages of at most 24 *real emotes*; no installed emote is omitted.
const MAX_EMOTES_PER_RING: usize = 24;
const MAX_BROWSE_SECTIONS: usize = 12;

/// Session-only keyboard browser. A pinned browser must never synthesize
/// +movement/+attack or trap the mouse cursor like the regular radial wheel.
#[derive(Default)]
pub(in crate::app) struct EmoteRadialSearch {
    pub(in crate::app) pinned: bool,
    pub(in crate::app) query: String,
    pub(in crate::app) category: String,
    pub(in crate::app) favorites_only: bool,
    pub(in crate::app) selection: usize,
    pub(in crate::app) request_focus: bool,
    pub(in crate::app) scroll_to_selection: bool,
    cached_query: String,
    cached_category: String,
    cached_favorites_only: bool,
    cached_catalog_ptr: usize,
    cached_hits: std::sync::Arc<Vec<SearchHit>>,
}

#[derive(Clone, Debug)]
struct SearchHit {
    id: String,
    wire_id: Option<u16>,
    title: String,
    category: String,
    favorite: bool,
    rank: u8,
}

/// Match all query words against the displayed title or stable animation ID.
/// Give exact/prefix title matches priority over arbitrary substrings.
fn emote_search_rank(title: &str, id: &str, query: &str) -> Option<u8> {
    let title = title.to_lowercase();
    let id = id.to_lowercase();
    let query = query.trim().to_lowercase();
    if query.is_empty() { return Some(3); }
    if !query.split_whitespace().all(|term| title.contains(term) || id.contains(term)) {
        return None;
    }
    Some(if title == query { 0 }
        else if title.starts_with(&query) { 1 }
        else if title.contains(&query) { 2 }
        else { 3 })
}


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum EmoteCategory {
    Taunts,
    Standard,
    Library,
    LibraryCategory(usize),
    LibraryGroup(usize, usize),
    LibraryPage(usize, usize, usize),
    Favorites,
}

impl EmoteCategory {
    fn parent(self) -> Option<Self> {
        match self {
            Self::LibraryCategory(_) => Some(Self::Library),
            Self::LibraryGroup(category, _) => Some(Self::LibraryCategory(category)),
            Self::LibraryPage(category, group, _) => Some(Self::LibraryGroup(category, group)),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Action {
    Category(EmoteCategory),
    Command(&'static str),
    /// An installed emote, by catalog ID.
    Emote(String),
    Match,
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

fn categories() -> [Entry; 5] {
    [
        Entry::new("TAUNTS", "SABER + GESTURES", Action::Category(EmoteCategory::Taunts)),
        Entry::new("STANDARD", "SERVER MOD", Action::Category(EmoteCategory::Standard)),
        Entry::new("LIBRARY", "BROWSE CATEGORIES", Action::Category(EmoteCategory::Library)),
        Entry::new("FAVORITES", "STARRED EMOTES", Action::Category(EmoteCategory::Favorites)),
        Entry::new("MATCH", "SYNC WITH AIMED PLAYER", Action::Match),
    ]
}

/// Every emote in one uninterrupted ring: no Back, Prev, Next, or page slots.
/// Dynamic tiers contain library categories, installed emotes, or favorites.
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
        EmoteCategory::Library | EmoteCategory::LibraryCategory(_) | EmoteCategory::LibraryGroup(_, _) | EmoteCategory::LibraryPage(_, _, _) | EmoteCategory::Favorites => library,
    }
}

/// Installed emotes or favorites, or a disabled entry explaining an empty tier.
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

// Split an alphabetically ordered category into at most 12 equally sized
// sections. A section with more than 24 emotes gets one final page tier.
// Integer partitioning guarantees that all positions are covered exactly once.
fn category_sections(len: usize) -> Vec<std::ops::Range<usize>> {
    if len == 0 { return Vec::new(); }
    if len <= MAX_EMOTES_PER_RING { return vec![0..len]; }
    let count = len.div_ceil(MAX_EMOTES_PER_RING).min(MAX_BROWSE_SECTIONS);
    (0..count).map(|i| len * i / count .. len * (i + 1) / count).collect()
}

fn section_pages(section: std::ops::Range<usize>) -> Vec<std::ops::Range<usize>> {
    (section.start..section.end).step_by(MAX_EMOTES_PER_RING)
        .map(|start| start..(start + MAX_EMOTES_PER_RING).min(section.end)).collect()
}

fn category_wheel_label(category: &super::emote_wheel::Category) -> String {
    // Short built-in labels keep a 14-way category wheel readable, while a
    // custom name in Profile is always respected verbatim.
    let short = match (category.id.as_str(), category.name.as_str()) {
        ("dance-street", "Street & Hip-Hop") => Some("STREET"),
        ("dance-pop", "Pop & Choreo") => Some("POP"),
        ("dance-party", "Party & Club") => Some("PARTY"),
        ("dance-goofy", "Goofy & Meme") => Some("GOOFY"),
        ("dance", "Other Dances") => Some("DANCES"),
        ("music", "Music & Instruments") => Some("MUSIC"),
        ("taunt", "Taunts & Trolling") => Some("TAUNTS"),
        ("celebrate", "Celebrations & Victory") => Some("VICTORY"),
        ("social", "Greetings & Social") => Some("SOCIAL"),
        ("reaction", "Reactions & Moods") => Some("MOODS"),
        ("flex", "Poses & Flex") => Some("POSES"),
        ("action", "Action & Acrobatics") => Some("ACTION"),
        ("props", "Props & Activities") => Some("PROPS"),
        ("misc", "Other Emotes") => Some("OTHER"),
        _ => None,
    };
    short.map(str::to_owned).unwrap_or_else(|| category.name.to_uppercase())
}

fn short_initial(emote: &crate::emotes::Emote) -> String {
    crate::emotes::display_title(emote).trim_start().chars()
        .filter(|c| c.is_alphanumeric()).take(2)
        .collect::<String>().to_uppercase()
}

fn range_entry(ordered: &[&crate::emotes::Emote], range: std::ops::Range<usize>, index: usize, count: usize, action: Action) -> Entry {
    let first = short_initial(ordered[range.start]);
    let last = short_initial(ordered[range.end - 1]);
    Entry {
        label: Cow::Owned(format!("{:02}  {}–{}", index + 1, first, last)),
        sub: Cow::Owned(format!("{} EMOTES · {}/{}", range.len(), index + 1, count)),
        action,
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

/// The same user-authored category tabs are shown over both Fortnite lists.
/// Selecting a tab switches the filter instead of stacking hidden checkboxes.
pub(in crate::app) fn emote_category_tabs(
    ui: &mut egui::Ui,
    categories: &[crate::app::emote_wheel::Category],
    category: &mut String,
    favorites_only: &mut bool,
) -> bool {
    let previous = (category.clone(), *favorites_only);
    egui::ScrollArea::horizontal().id_salt(ui.id().with("fortnite_category_tabs"))
        .auto_shrink([false, true]).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            if ui.add(egui::Button::new("All").selected(category.is_empty() && !*favorites_only)).clicked() {
                category.clear(); *favorites_only = false;
            }
            if ui.add(egui::Button::new("★ Favorites").selected(*favorites_only)).clicked() {
                category.clear(); *favorites_only = true;
            }
            for c in categories {
                if ui.add(egui::Button::new(c.name.as_str()).selected(category.as_str() == c.id.as_str() && !*favorites_only))
                    .on_hover_text(format!("Only {} emotes", c.name)).clicked() {
                    *category = c.id.clone(); *favorites_only = false;
                }
            }
        });
    });
    previous.0.as_str() != category.as_str() || previous.1 != *favorites_only
}

impl App {
    /// Every installed Fortnite emote is searchable, not just favorites or
    /// the current radial category. Keep only lightweight labels/IDs per hit.
    fn emote_radial_search_hits(&mut self) -> std::sync::Arc<Vec<SearchHit>> {
        let Ok(catalog) = crate::emotes::catalog(&self.base) else {
            self.emote_radial_search.cached_catalog_ptr = 0;
            return std::sync::Arc::new(Vec::new());
        };
        let ptr = std::sync::Arc::as_ptr(&catalog) as usize;
        if self.emote_radial_search.cached_catalog_ptr == ptr
            && self.emote_radial_search.cached_query == self.emote_radial_search.query
            && self.emote_radial_search.cached_category == self.emote_radial_search.category
            && self.emote_radial_search.cached_favorites_only == self.emote_radial_search.favorites_only
        {
            return self.emote_radial_search.cached_hits.clone();
        }
        let mut hits: Vec<SearchHit> = catalog.iter().filter_map(|root| {
            let variant_index = self.emote_wheel.variants.get(&root.id).copied()
                .unwrap_or_else(|| crate::emotes::default_variant_index(root));
            let selected = root.variants.get(variant_index).unwrap_or(root);
            let title = crate::emotes::display_title(selected);
            let rank = emote_search_rank(title, &selected.id, &self.emote_radial_search.query)
                .or_else(|| emote_search_rank(crate::emotes::display_title(root), &root.id,
                    &self.emote_radial_search.query))?;
            let category_id = self.emote_wheel.category_for(root);
            let category = self.emote_wheel.categories.iter()
                .find(|c| c.id == category_id).map(|c| c.name.clone())
                .unwrap_or_else(|| "Other Emotes".into());
            let favorite = std::iter::once(&root.id).chain(root.catalog_aliases.iter())
                .chain(root.variants.iter().map(|v| &v.id))
                .any(|id| self.emote_wheel.favorites.contains(id));
            if (self.emote_radial_search.favorites_only && !favorite)
                || (!self.emote_radial_search.category.is_empty()
                    && self.emote_radial_search.category != category_id) { return None; }
            Some(SearchHit {
                id: selected.id.clone(), wire_id: crate::emotes::network::wire_id(selected),
                title: title.to_owned(), category, favorite, rank,
            })
        }).collect();
        hits.sort_by(|a, b| a.rank.cmp(&b.rank)
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id)));
        self.emote_radial_search.cached_query = self.emote_radial_search.query.clone();
        self.emote_radial_search.cached_category = self.emote_radial_search.category.clone();
        self.emote_radial_search.cached_favorites_only = self.emote_radial_search.favorites_only;
        self.emote_radial_search.cached_catalog_ptr = ptr;
        self.emote_radial_search.cached_hits = std::sync::Arc::new(hits);
        self.emote_radial_search.cached_hits.clone()
    }

    pub(in crate::app) fn emote_radial_search_move(&mut self, step: isize) {
        if !self.emote_radial_search.pinned { return; }
        let count = self.emote_radial_search_hits().len();
        if count == 0 { return; }
        let current = self.emote_radial_search.selection.min(count - 1);
        self.emote_radial_search.selection = current.saturating_add_signed(step).min(count - 1);
        self.emote_radial_search.scroll_to_selection = true;
        self.egui_repaint_requested = true;
    }

    pub(in crate::app) fn activate_emote_radial_search_selection(&mut self) {
        if !self.emote_radial_search.pinned { return; }
        let hits = self.emote_radial_search_hits();
        if let Some(hit) = hits.get(self.emote_radial_search.selection) {
            // Unlike the hold-to-open wheel, pinned browsing stays visible so
            // another emote can be previewed/played without reopening it.
            self.play_wheel_emote(&hit.id);
        }
    }

    /// A pinned wheel becomes an egui text browser with a real, unlocked
    /// pointer. Virtualized results keep a 1,800+ item catalog responsive.
    fn egui_pinned_emote_search(&mut self, root: &mut egui::Ui) {
        let rect = root.max_rect();
        let mut selected_id = None;
        let mut unpin = false;
        let mut close = false;
        // Non-modal right rail: no full-screen scrim and no center overlay.
        // Leave the world and player's dance visible while clicking emotes.
        let margin = 12.0;
        let width = (rect.width() * 0.30).clamp(255.0, 385.0).min(rect.width() - margin * 2.0);
        let available_h = (rect.height() - margin * 2.0).max(100.0);
        let mut category = self.emote_radial_search.category.clone();
        let mut favorites_only = self.emote_radial_search.favorites_only;
        egui::Area::new(egui::Id::new("pinned_emote_right_rail"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-margin, margin))
            .order(egui::Order::Foreground)
            .show(root.ctx(), |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_unmultiplied(16, 23, 33, 238))
                    .stroke(egui::Stroke::new(1.0, theme::LINE_STRONG))
                    .corner_radius(egui::CornerRadius::same(9))
                    .inner_margin(10.0)
                    .show(ui, |ui| {
                        ui.set_width(width - 20.0);
                        ui.set_max_width(width - 20.0);
                        ui.horizontal(|ui| {
                            ui.strong("FORTNITE EMOTES");
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button("×").on_hover_text("Close").clicked() { close = true; }
                                if ui.small_button("Wheel").on_hover_text("Unpin / return to wheel").clicked() { unpin = true; }
                            });
                        });
                        let search = ui.add(egui::TextEdit::singleline(&mut self.emote_radial_search.query)
                            .hint_text("Search emotes or EID…")
                            .desired_width(f32::INFINITY));
                        if self.emote_radial_search.request_focus {
                            search.request_focus();
                            self.emote_radial_search.request_focus = false;
                        }
                        if search.changed() {
                            self.emote_radial_search.selection = 0;
                            self.emote_radial_search.scroll_to_selection = true;
                        }
                        ui.add_space(4.0);
                        let changed = emote_category_tabs(ui, &self.emote_wheel.categories, &mut category, &mut favorites_only);
                        if changed {
                            self.emote_radial_search.category = category.clone();
                            self.emote_radial_search.favorites_only = favorites_only;
                            self.emote_radial_search.selection = 0;
                            self.emote_radial_search.scroll_to_selection = true;
                        }
                        let hits = self.emote_radial_search_hits();
                        self.emote_radial_search.selection = self.emote_radial_search.selection.min(hits.len().saturating_sub(1));
                        ui.small(format!("{} matches · ↑↓ select · Enter play · Tab wheel", hits.len()));
                        ui.separator();
                        if hits.is_empty() {
                            ui.label("No matching emotes in this category.");
                        } else {
                            let scroll_to_selection = std::mem::take(&mut self.emote_radial_search.scroll_to_selection);
                            let mut area = egui::ScrollArea::vertical().id_salt("pinned_emote_results")
                                .auto_shrink([false, false])
                                .max_height((available_h - 150.0).max(110.0));
                            if scroll_to_selection {
                                area = area.vertical_scroll_offset(self.emote_radial_search.selection as f32 * 34.0);
                            }
                            area.show_rows(ui, 30.0, hits.len(), |ui, range| {
                                for index in range {
                                    let hit = &hits[index];
                                    let id_label = hit.wire_id.map_or_else(
                                        || "LOCAL".to_owned(), |id| format!("#{id}"));
                                    let text = format!("{}  {}{}", id_label,
                                        if hit.favorite { "★ " } else { "" }, hit.title);
                                    let row = ui.add_sized([ui.available_width(), 30.0],
                                        egui::Button::new(text).selected(self.emote_radial_search.selection == index));
                                    if row.clicked() {
                                        self.emote_radial_search.selection = index;
                                        selected_id = Some(hit.id.clone());
                                    }
                                    let command = hit.wire_id.map_or_else(
                                        || format!("djk_emote {}", hit.id),
                                        |id| format!("djk_emote {id}"));
                                    row.on_hover_text(format!("{} · {}\n{}", hit.category, hit.id, command));
                                }
                            });
                        }
                        ui.separator();
                        ui.small("Click to dance; panel stays open · Esc closes");
                    });
            });
        if close { self.close_emote_radial(); }
        else if unpin { self.toggle_emote_radial_search(); }
        else if let Some(id) = selected_id { self.play_wheel_emote(&id); }
    }

    pub(in crate::app) fn emote_radial_back(&mut self) {
        if self.emote_radial_category.is_some() {
            self.emote_radial_category = self.emote_radial_category.and_then(EmoteCategory::parent);
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
                if category == EmoteCategory::Favorites {
                    self.warm_favorite_emote_audio();
                }
            }
            Action::Command(command) => {
                self.close_emote_radial();
                self.append_console_commands(vec![command.to_owned()]);
            }
            Action::Emote(id) => {
                self.close_emote_radial();
                self.play_wheel_emote(&id);
            }
            Action::Match => self.match_crosshair_emote(),
            Action::Disabled => {}
        }
    }

    fn radial_library_entries(&self, category: EmoteCategory) -> Vec<Entry> {
        if category == EmoteCategory::Library {
            let counts = crate::emotes::catalog(&self.base).ok().map(|catalog| {
                let mut counts = std::collections::HashMap::<String, usize>::new();
                for emote in catalog.iter() {
                    *counts.entry(self.emote_wheel.category_for(emote)).or_default() += 1;
                }
                counts
            });
            return self.emote_wheel.categories.iter().enumerate().map(|(index, category)| Entry {
                label: Cow::Owned(category_wheel_label(category)),
                sub: Cow::Owned(match &counts {
                    Some(counts) => format!("{} · {} EMOTES", category.name.to_uppercase(),
                        counts.get(&category.id).copied().unwrap_or(0)),
                    None => format!("{} · LOADING", category.name.to_uppercase()),
                }),
                action: Action::Category(EmoteCategory::LibraryCategory(index)),
            }).collect();
        }
        if category == EmoteCategory::Favorites {
            return library_entries(self.wheel_favorite_emotes());
        }
        let category_index = match category {
            EmoteCategory::LibraryCategory(index)
            | EmoteCategory::LibraryGroup(index, _)
            | EmoteCategory::LibraryPage(index, _, _) => index,
            _ => return Vec::new(),
        };
        let Some(selected_category) = self.emote_wheel.categories.get(category_index) else {
            return library_entries(Err("CATEGORY UNAVAILABLE"));
        };
        let Ok(catalog) = crate::emotes::catalog(&self.base) else {
            return library_entries(Err("EMOTE LIBRARY LOADING OR UNAVAILABLE"));
        };
        // Keep source references instead of cloning every animation/prop/pose
        // just to build a list of browse labels at the menu refresh rate.
        // The catalogue is already title-sorted in emotes::group_titles;
        // retain that order instead of re-sorting thousands of rows each frame.
        let ordered: Vec<_> = catalog.iter()
            .filter(|emote| self.emote_wheel.category_for(emote) == selected_category.id)
            .collect();
        if ordered.is_empty() {
            return library_entries(Err("NO EMOTES IN THIS CATEGORY"));
        }
        let sections = category_sections(ordered.len());
        let display_range = match category {
            EmoteCategory::LibraryCategory(index) if ordered.len() > MAX_EMOTES_PER_RING => {
                return sections.iter().enumerate().map(|(i, range)| {
                    range_entry(&ordered, range.clone(), i, sections.len(),
                        Action::Category(EmoteCategory::LibraryGroup(index, i)))
                }).collect();
            }
            EmoteCategory::LibraryCategory(_) => 0..ordered.len(),
            EmoteCategory::LibraryGroup(index, group) => {
                let Some(section) = sections.get(group) else {
                    return library_entries(Err("SECTION UNAVAILABLE"));
                };
                let pages = section_pages(section.clone());
                if pages.len() > 1 {
                    return pages.iter().enumerate().map(|(i, range)| {
                        range_entry(&ordered, range.clone(), i, pages.len(),
                            Action::Category(EmoteCategory::LibraryPage(index, group, i)))
                    }).collect();
                }
                section.clone()
            }
            EmoteCategory::LibraryPage(_, group, page) => {
                let Some(section) = sections.get(group) else {
                    return library_entries(Err("SECTION UNAVAILABLE"));
                };
                let pages = section_pages(section.clone());
                let Some(range) = pages.get(page) else {
                    return library_entries(Err("PAGE UNAVAILABLE"));
                };
                range.clone()
            }
            _ => unreachable!(),
        };
        ordered[display_range].iter().map(|root| {
            let variant = self.emote_wheel.variants.get(&root.id).copied()
                .unwrap_or_else(|| crate::emotes::default_variant_index(root));
            let selected = root.variants.get(variant).unwrap_or(root);
            Entry {
                label: Cow::Owned(crate::emotes::display_title(selected).to_uppercase()),
                sub: Cow::Owned(selected.id.clone()),
                action: Action::Emote(selected.id.clone()),
            }
        }).collect()
    }

    pub(super) fn egui_emote_radial(&mut self, root: &mut egui::Ui) {
        if self.emote_radial_search.pinned {
            self.egui_pinned_emote_search(root);
            return;
        }
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
            Some(selected_sector(self.dev_radial_offset, categories().len()))
        } else { None };

        painter.rect_filled(rect, egui::CornerRadius::ZERO, egui::Color32::from_black_alpha(92));
        painter.circle_filled(center, outer + 11.0, egui::Color32::from_black_alpha(68));

        // Paint the entire outer catalog, with radial text so every narrow
        // slice has an individually visible full label even with 30+ emotes.
        // Keep the Library categories in their own middle band when the
        // selected category's emotes open in a third, separate outer band.
        let library_categories = if matches!(chosen_category, Some(EmoteCategory::LibraryCategory(_)
            | EmoteCategory::LibraryGroup(_, _) | EmoteCategory::LibraryPage(_, _, _))) {
            self.radial_library_entries(EmoteCategory::Library)
        } else { Vec::new() };
        let mut rings = Vec::with_capacity(2);
        match chosen_category {
            Some(EmoteCategory::Library) => {
                if let Some(items) = catalog {
                    rings.push((items, outer * EMOTE_INNER, outer * LIBRARY_CATEGORY_OUTER, selected_outer));
                }
            }
            Some(EmoteCategory::LibraryCategory(index) | EmoteCategory::LibraryGroup(index, _)
                | EmoteCategory::LibraryPage(index, _, _)) => {
                rings.push((library_categories.as_slice(), outer * EMOTE_INNER,
                    outer * LIBRARY_CATEGORY_OUTER, Some(index)));
                if let Some(items) = catalog {
                    rings.push((items, outer * LIBRARY_EMOTE_INNER, outer, selected_outer));
                }
            }
            _ => {
                if let Some(items) = catalog {
                    rings.push((items, outer * EMOTE_INNER, outer, selected_outer));
                }
            }
        }
        for (items, inner, ring_outer, selected_ring) in rings {
            if !items.is_empty() {
                let step = 2.0 * PI / items.len() as f32;
                let width = ring_outer - inner;
                // Aim the label toward the middle of the band, leaving room
                // around both ends to avoid touching the inner ring.
                let label_radius = inner + width * 0.5;
                for (i, choice) in items.iter().enumerate() {
                    let angle = -FRAC_PI_2 + i as f32 * step;
                    let visual_gap = SECTOR_GAP.min(step * 0.16);
                    let start = angle - step * 0.5 + visual_gap;
                    let end = angle + step * 0.5 - visual_gap;
                    let highlighted = selected_ring == Some(i) && choice.enabled();
                    let fill = if highlighted {
                        egui::Color32::from_rgba_unmultiplied(38, 94, 151, 242)
                    } else if choice.enabled() {
                        egui::Color32::from_rgba_unmultiplied(17, 25, 37, 235)
                    } else {
                        egui::Color32::from_rgba_unmultiplied(24, 30, 39, 219)
                    };
                    paint_annular_sector(painter, center, inner, ring_outer, start, end, fill);
                    paint_arc(painter, center, ring_outer, start, end,
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
            let label_radius = (ring_inner + ring_outer) * 0.5;
            let size = emote_font_size(painter, &choice.label, (ring_outer - ring_inner - 12.0).max(12.0));
            paint_radial_label(painter, center, angle, label_radius, &choice.label, size, fg);
        }

        painter.circle_filled(center, ring_inner - 3.0, theme::CHROME);
        painter.circle_stroke(center, ring_inner - 2.0, egui::Stroke::new(1.0, theme::LINE_STRONG));
        painter.text(center, egui::Align2::CENTER_CENTER, "EMOTE",
            egui::FontId::proportional(if outer > 300.0 { 15.0 } else { 10.0 }), theme::TEXT);

        let heading = match chosen_category {
            Some(EmoteCategory::Library) => "LIBRARY / CATEGORIES".to_owned(),
            Some(EmoteCategory::LibraryCategory(index)) => self.emote_wheel.categories.get(index)
                .map(|c| format!("LIBRARY / {}", c.name.to_uppercase())).unwrap_or_default(),
            Some(EmoteCategory::LibraryGroup(index, group)) => self.emote_wheel.categories.get(index)
                .map(|c| format!("{} / SECTION {}", c.name.to_uppercase(), group + 1)).unwrap_or_default(),
            Some(EmoteCategory::LibraryPage(index, group, page)) => self.emote_wheel.categories.get(index)
                .map(|c| format!("{} / SECTION {} / PAGE {}", c.name.to_uppercase(), group + 1, page + 1)).unwrap_or_default(),
            Some(EmoteCategory::Favorites) => "FAVORITES".to_owned(),
            _ => String::new(),
        };
        painter.text(center - egui::vec2(0.0, outer + 18.0), egui::Align2::CENTER_CENTER,
            heading, egui::FontId::proportional(12.0), theme::TEXT);

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
            if chosen_category.is_some() { "LEFT CLICK SELECT  ·  RIGHT CLICK BACK  ·  TAB PIN + SEARCH  ·  RELEASE CANCELS" }
            else { "LEFT CLICK OPEN  ·  TAB PIN + SEARCH  ·  RELEASE CANCELS" },
            egui::FontId::proportional(9.0), theme::TEXT_DIM);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_is_case_insensitive_and_ranked() {
        assert_eq!(emote_search_rank("Floss", "EID_Floss", "FLOSS"), Some(0));
        assert_eq!(emote_search_rank("Flossing", "EID_Flossing", "flo"), Some(1));
        assert_eq!(emote_search_rank("Windmill Floss", "EID_WindmillFloss", "floss"), Some(2));
        assert_eq!(emote_search_rank("Dance", "EID_OrangeJustice", "orange"), Some(3));
        assert_eq!(emote_search_rank("Dance", "EID_Wave", "no match"), None);
        assert_eq!(emote_search_rank("Windmill Floss", "EID_Windmill", "wind floss"), Some(3));
    }

    use std::collections::HashSet;

    #[test]
    fn huge_library_has_small_complete_non_overlapping_browse_rings() {
        for size in [1, 24, 25, 100, 288, 1852, 4500] {
            let sections = category_sections(size);
            assert!(sections.len() <= MAX_BROWSE_SECTIONS);
            let mut recovered = Vec::new();
            for section in sections {
                let pages = section_pages(section);
                assert!(pages.len() <= MAX_EMOTES_PER_RING);
                for range in pages {
                    assert!(range.len() <= MAX_EMOTES_PER_RING);
                    recovered.extend(range);
                }
            }
            assert_eq!(recovered, (0..size).collect::<Vec<_>>());
        }
    }

    #[test]
    fn nested_library_backtracks_one_level_at_a_time() {
        assert_eq!(EmoteCategory::LibraryPage(3, 2, 5).parent(), Some(EmoteCategory::LibraryGroup(3, 2)));
        assert_eq!(EmoteCategory::LibraryGroup(3, 2).parent(), Some(EmoteCategory::LibraryCategory(3)));
        assert_eq!(EmoteCategory::LibraryCategory(3).parent(), Some(EmoteCategory::Library));
    }

    #[test]
    fn no_navigation_sectors_and_complete_standard_catalogs() {
        assert_eq!(categories().len(), 5);
        assert_eq!(categories()[3].action, Action::Category(EmoteCategory::Favorites));
        assert_eq!(categories()[4].action, Action::Match);
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
    fn favorites_tier_lists_starred_emotes_or_says_why_it_is_empty() {
        let empty = library_entries(Err("STAR EMOTES IN PROFILE"));
        assert_eq!(empty.len(), 1);
        assert!(!empty[0].enabled());
        let emotes = jka_emotes::manifest::parse_manifest(&serde_json::json!({"emotes":[
            {"id":"EID_Floss","name":"Floss","gla":"models/b.gla","first_frame":0,"end_frame":30,"fps":30}]})).unwrap();
        let tier = library_entries(Ok(emotes));
        assert_eq!(tier[0].label, "FLOSS");
        assert_eq!(tier[0].action, Action::Emote("EID_Floss".into()));
        assert_eq!(entries(EmoteCategory::Favorites, ServerMod::Base, &tier).len(), 1);
    }

    #[test]
    fn library_back_returns_through_each_layer() {
        assert_eq!(EmoteCategory::LibraryCategory(2).parent(), Some(EmoteCategory::Library));
        assert_eq!(EmoteCategory::Library.parent(), None);
        assert_eq!(EmoteCategory::Favorites.parent(), None);
        assert_eq!(EmoteCategory::Taunts.parent(), None);
        assert_eq!(EmoteCategory::Standard.parent(), None);
    }

    #[test]
    fn sectors_are_stable_with_large_catalogs() {
        assert_eq!(selected_sector([0.0, -0.28], 5), 0);
        for n in [4, 5, 29, 32] {
            for i in 0..n {
                let angle = -FRAC_PI_2 + i as f32 * 2.0 * PI / n as f32;
                assert_eq!(selected_sector([angle.cos(), angle.sin()], n), i);
            }
        }
    }
}
