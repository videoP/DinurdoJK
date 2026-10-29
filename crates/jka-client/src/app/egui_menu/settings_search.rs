//! Global, read-only Settings search. Results route to the existing controls;
//! changing a setting still goes through its original page and setter.
use super::*;

#[derive(Clone, Copy)]
struct SearchItem {
    tab: usize,
    section: Option<VideoSection>,
    label: &'static str,
    hint: &'static str,
    alias: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/settings_search_index.rs"));

fn query_id() -> egui::Id { egui::Id::new("jka_settings_search_text") }

pub(in crate::app) fn clear_query(ctx: &egui::Context) {
    ctx.data_mut(|data| data.remove::<String>(query_id()));
}

pub(in crate::app) fn current_query(ctx: &egui::Context) -> String {
    ctx.data(|data| data.get_temp::<String>(query_id()).unwrap_or_default())
}

fn category(item: SearchItem) -> &'static str {
    if let Some(section) = item.section {
        match section {
            VideoSection::Display => "Display & graphics",
            VideoSection::Geometry => "Geometry",
            VideoSection::Lighting => "Lighting",
            VideoSection::Relighting => "Relighting",
            VideoSection::Materials => "Material rendering",
            VideoSection::Effects => "Effects",
            VideoSection::PostProcessing => "Post processing",
            VideoSection::Color => "Color & HDR",
            VideoSection::DebugTools => "Debug & tools",
            VideoSection::Physics => "Physics",
            VideoSection::Sun => "Sky",
            VideoSection::BlueWater | VideoSection::Water => "Ocean",
            VideoSection::Clouds => "Clouds",
            VideoSection::Weather => "Weather",
            VideoSection::Surface => "Surface interaction",
            VideoSection::Glass => "Glass",
            VideoSection::Optimizations => "Optimizations",
        }
    } else {
        match item.tab {
            SETUP_TAB_GAME => "Game",
            SETUP_TAB_MOVEMENT => "Movement",
            SETUP_TAB_CAMERA => "Camera",
            SETUP_TAB_AUDIO => "Audio",
            SETUP_TAB_NETWORK => "Network",
            SETUP_TAB_INTERFACE => "Interface",
            SETUP_TAB_EXTENSIONS => "Extensions",
            SETUP_TAB_STREAMER => "Streamer mode",
            SETUP_TAB_PK3 => "PK3 Manager",
            SETUP_TAB_MOD => "Mod",
            _ => "Video",
        }
    }
}

fn path(item: SearchItem) -> String {
    if item.tab == SETUP_TAB_VIDEO {
        format!("VIDEO  /  {}", category(item))
    } else {
        category(item).to_ascii_uppercase()
    }
}

fn score(item: SearchItem, tokens: &[String], joined: &str) -> Option<i32> {
    let label = item.label.to_ascii_lowercase();
    let hint = item.hint.to_ascii_lowercase();
    let alias = item.alias.to_ascii_lowercase();
    let location = path(item).to_ascii_lowercase();
    if !tokens.iter().all(|token| label.contains(token) || hint.contains(token) || alias.contains(token) || location.contains(token)) {
        return None;
    }
    let mut score = 0;
    if label == joined { score += 1000; }
    else if label.starts_with(joined) { score += 650; }
    else if label.contains(joined) { score += 400; }
    for token in tokens {
        if label.split(|c: char| !c.is_alphanumeric()).any(|word| word == token) { score += 80; }
        else if label.contains(token) { score += 45; }
        else if alias.contains(token) { score += 35; }
        else if hint.contains(token) { score += 12; }
        else { score += 3; }
    }
    Some(score)
}

/// Always visible while Setup is open. On the Video tab this lives inside the
/// existing pinned preset toolbar; elsewhere it is pinned above each page.
pub(in crate::app) fn search_bar(ui: &mut egui::Ui) -> bool {
    let mut query = current_query(ui.ctx());
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("SEARCH SETTINGS").size(11.5).color(theme::TEXT_DIM));
        let width = (ui.available_width() - 58.0).max(100.0);
        let response = ui.add_sized(
            [width, 27.0],
            egui::TextEdit::singleline(&mut query)
                .id_salt("jka_settings_search_field")
                .hint_text("Name, feature, or cvar (e.g. r_fragmentShaderMode)"),
        );
        if ui.input(|input| input.modifiers.ctrl && input.key_pressed(egui::Key::F)) {
            response.request_focus();
        }
        if ui.small_button("CLEAR").clicked() {
            query.clear();
            ui.memory_mut(|memory| memory.surrender_focus(response.id));
        }
        if response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            query.clear();
            ui.memory_mut(|memory| memory.surrender_focus(response.id));
        }
    });
    let active = !query.trim().is_empty();
    ui.ctx().data_mut(|data| data.insert_temp(query_id(), query));
    active
}

impl App {
    /// Search is a temporary all-pages view, not a replacement settings editor.
    pub(in crate::app) fn egui_settings_search_results(&mut self, ui: &mut egui::Ui) {
        let query = current_query(ui.ctx());
        let joined = query.trim().to_ascii_lowercase();
        let tokens: Vec<String> = joined.split_whitespace().map(str::to_owned).collect();
        if tokens.is_empty() { return; }
        let mut results: Vec<(i32, SearchItem)> = SEARCH_ITEMS.iter().copied()
            .filter_map(|item| score(item, &tokens, &joined).map(|score| (score, item)))
            .filter(|(_, item)| item.tab != SETUP_TAB_MOD || self.ui_mod().is_some())
            .collect();
        // Inside Render Studio the search runs in the Studio graphics window,
        // which shows graphics pages only: other matches can't be opened there.
        let hidden = if self.render_studio.graphics_gameplay.is_some() {
            let before = results.len();
            results.retain(|(_, item)| item.tab == SETUP_TAB_VIDEO);
            before - results.len()
        } else { 0 };
        results.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.label.cmp(b.1.label)));
        theme::page_title(ui, "SEARCH RESULTS", &format!("{} matching settings across Setup", results.len()));
        if hidden > 0 {
            theme::hint(ui, &format!("{hidden} gameplay / interface setting(s) also match. They live in Setup, outside \
                Render Studio. The Studio camera (third person, FOV) is set on camera keys and Director shots."), theme::WARNING);
        }
        let mut chosen = None;
        egui::ScrollArea::vertical()
            .id_salt("jka_settings_search_results")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if results.is_empty() {
                    ui.add_space(12.0);
                    ui.label(egui::RichText::new("No matching settings. Try a shorter name or a cvar.").color(theme::TEXT_DIM));
                }
                for &(_, item) in results.iter().take(150) {
                    let description = item.hint.split('.').next().unwrap_or("").trim();
                    let preview = description.chars().take(115).collect::<String>();
                    let preview = if item.alias.is_empty() { preview } else { format!("{}  ({})", preview, item.alias) };
                    let response = ui.add_sized(
                        [ui.available_width(), 54.0],
                        egui::Button::new(format!("{}\n{}  ·  {}", item.label, path(item), preview))
                            .fill(theme::CONTROL),
                    );
                    if response.clicked() { chosen = Some(item); }
                    ui.add_space(2.0);
                }
                if results.len() > 150 {
                    ui.label(egui::RichText::new("Showing the first 150 matches. Type more to narrow the search.").color(theme::TEXT_FAINT));
                }
            });
        if let Some(item) = chosen {
            self.setup_selected = item.tab;
            if let Some(section) = item.section { self.video_section = section; }
            clear_query(ui.ctx());
            theme::jump_to_setting(ui.ctx(), item.label);
            self.egui_repaint_requested = true;
            self.publish_ui();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_and_cvars_are_searchable() {
        let items = SEARCH_ITEMS;
        assert!(items.len() > 100, "index should cover the actual Settings pages");
        assert!(items.iter().any(|item| item.label == "Anti-aliasing"));
        assert!(items.iter().any(|item| item.label == "Player movement rate"));
        assert!(items.iter().any(|item| {
            item.alias == "r_fragmentShaderMode" && score(*item, &["r_fragmentshadermode".to_owned()], "r_fragmentshadermode").is_some()
        }));
    }
}
