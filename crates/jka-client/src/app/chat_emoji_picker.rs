//! Searchable emoji button over either native chat composer. Only the button
//! and popup use egui; keyboard editing, wire encoding and the chat HUD stay
//! with the existing WGPU editor.
use super::{App, OverlayMode};
use std::{collections::{HashMap, VecDeque}, path::{Path, PathBuf}, sync::{Arc, mpsc}};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::app) enum PickerTab { #[default] Legacy, Unicode }

type EmojiIconBatch = Vec<(char, Option<crate::ui::UnicodeGlyphBitmap>)>;

pub(in crate::app) struct EmojiPickerCatalog {
    legacy: Option<crate::renderer::chat_emoji::ChatPickerLegacyAtlas>,
    unicode: Vec<char>,
    font: Option<Arc<Vec<u8>>>,
}

#[derive(Default)]
pub(in crate::app) struct ChatEmojiPicker {
    pub open: bool,
    tab: PickerTab,
    search: String,
    scope: Option<(Option<PathBuf>, Option<u8>)>,
    loaded: bool,
    job: Option<mpsc::Receiver<EmojiPickerCatalog>>,
    catalog: Option<EmojiPickerCatalog>,
    legacy_texture: Option<egui::TextureHandle>,
    icons: HashMap<char, Option<egui::TextureHandle>>,
    icon_order: VecDeque<char>,
    icon_job: Option<mpsc::Receiver<EmojiIconBatch>>,
}

impl ChatEmojiPicker {
    pub fn invalidate(&mut self) { *self = Self::default(); }

    pub fn forget_egui_textures(&mut self) {
        self.legacy_texture = None;
        self.icons.clear();
        self.icon_order.clear();
        self.icon_job = None;
    }

    pub fn can_show(&self, legacy_enabled: bool) -> bool {
        self.catalog.as_ref().is_some_and(|catalog|
            (legacy_enabled && catalog.legacy.is_some()) || !catalog.unicode.is_empty())
    }

    fn has_legacy(&self, enabled: bool) -> bool {
        enabled && self.catalog.as_ref().is_some_and(|catalog| catalog.legacy.is_some())
    }
    fn has_unicode(&self) -> bool {
        self.catalog.as_ref().is_some_and(|catalog| !catalog.unicode.is_empty())
    }
}

/// Select only actual mapped glyphs in the installed Noto color emoji font.
/// This includes all supported scalar emoji, not a hardcoded row of favorites;
/// composite ZWJ graphemes remain subject to the client's existing Unicode
/// renderer (which currently handles glyphs individually).
fn unicode_catalog(bytes: &[u8]) -> Vec<char> {
    let Ok(face) = ttf_parser::Face::parse(bytes, 0) else { return Vec::new(); };
    (0x00A9..=0x1FAFF)
        .filter_map(char::from_u32)
        .filter(|&ch| {
            let n = ch as u32;
            (crate::ui::is_emoji_candidate(ch)
                || matches!(n, 0x00A9 | 0x00AE | 0x203C | 0x2049 | 0x2122 | 0x2139
                    | 0x2194..=0x21FF | 0x2934..=0x2935 | 0x3030 | 0x303D | 0x3297 | 0x3299))
                && face.glyph_index(ch).is_some()
        })
        .collect()
}

fn load_catalog(base: &Path, game: Option<&Path>, jk2: Option<u8>) -> EmojiPickerCatalog {
    let legacy = crate::renderer::chat_emoji::load_chat_picker_atlas(base, game, jk2);
    let font = crate::ui::load_unicode_font("NotoColorEmoji.ttf", base, game).map(Arc::new);
    let unicode = font.as_deref().map_or_else(Vec::new, |bytes| unicode_catalog(bytes));
    EmojiPickerCatalog { legacy, unicode, font }
}

/// Search labels are generated from Unicode's official character names for
/// single codepoints. Unknown future codepoints are still selectable by hex.
fn unicode_name(ch: char) -> &'static str {
    static NAMES: std::sync::OnceLock<HashMap<u32, &'static str>> = std::sync::OnceLock::new();
    let names = NAMES.get_or_init(|| {
        include_str!("unicode_emoji_names.tsv").lines().filter_map(|line| {
            let (hex, name) = line.split_once('\t')?;
            Some((u32::from_str_radix(hex, 16).ok()?, name))
        }).collect()
    });
    names.get(&(ch as u32)).copied().unwrap_or("Unicode emoji")
}

impl App {
    pub(in crate::app) fn poll_chat_emoji_picker(&mut self) {
        if self.overlay != OverlayMode::Chat { return; }
        let scope = (self.game.clone(), self.fs_roots.active_jk2);
        if self.chat_emoji_picker.scope.as_ref() != Some(&scope) {
            self.chat_emoji_picker.invalidate();
            self.chat_emoji_picker.scope = Some(scope.clone());
        }
        let mut catalog_changed = false;
        let picker = &mut self.chat_emoji_picker;
        if let Some(job) = picker.job.as_ref() {
            match job.try_recv() {
                Ok(catalog) => {
                    picker.job = None;
                    picker.loaded = true;
                    picker.catalog = Some(catalog);
                    self.egui_repaint_requested = true;
                    catalog_changed = true;
                }
                Err(mpsc::TryRecvError::Empty) => {},
                Err(mpsc::TryRecvError::Disconnected) => picker.job = None,
            }
        }
        if !picker.loaded && picker.job.is_none() {
            let Some(pool) = crate::asset_jobs::pool() else { return; };
            let base = self.base.clone();
            let game = scope.0;
            let jk2 = scope.1;
            let (tx, rx) = mpsc::channel();
            if pool.submit_task(crate::asset_jobs::AssetPriority::Medium, move || {
                let _ = tx.send(load_catalog(&base, game.as_deref(), jk2));
            }) {
                picker.job = Some(rx);
            }
        }
        if catalog_changed { self.publish_ui(); }
    }

    /// Prepare color thumbnails from completed worker batches during an egui
    /// frame, so their texture deltas ship with the same frame as the grid.
    fn poll_picker_icon_textures(&mut self, ctx: &egui::Context) {
        let picker = &mut self.chat_emoji_picker;
        if let Some(job) = picker.icon_job.as_ref() {
            match job.try_recv() {
                Ok(batch) => {
                    picker.icon_job = None;
                    for (ch, bitmap) in batch {
                        let texture = bitmap.filter(|b| !b.rgba.is_empty()).map(|b| {
                            ctx.load_texture(format!("chat picker U+{:X}", ch as u32),
                                egui::ColorImage::from_rgba_unmultiplied(
                                    [b.glyph.width as usize, b.glyph.height as usize], &b.rgba),
                                egui::TextureOptions::LINEAR)
                        });
                        // Bound live bitmap handles; offscreen cells rehydrate on
                        // demand if the user scrolls far through Unicode.
                        if !picker.icons.contains_key(&ch) {
                            if picker.icons.len() >= 512 {
                                if let Some(old) = picker.icon_order.pop_front() { picker.icons.remove(&old); }
                            }
                            picker.icon_order.push_back(ch);
                        }
                        picker.icons.insert(ch, texture);
                    }
                    ctx.request_repaint();
                }
                Err(mpsc::TryRecvError::Empty) => {},
                Err(mpsc::TryRecvError::Disconnected) => picker.icon_job = None,
            }
        }
    }

    pub(in crate::app) fn chat_emoji_picker_visible(&self) -> bool {
        self.overlay == OverlayMode::Chat && self.chat_emoji_picker.can_show(self.chat_settings.emojis)
            && self.video_confirmation.is_none() && self.loading.is_none()
            && self.window.is_some() && self.render.is_some() && !self.video.skip_ui
    }

    pub(in crate::app) fn insert_picked_chat_emoji(&mut self, text: &str) {
        let extra = crate::cgame::unicode_input_wire_len(text);
        if self.chat_input_len_for_insert() + extra > super::chat::CHAT_INPUT_LIMIT {
            return;
        }
        self.insert_chat_at_cursor(text);
        self.publish_ui();
        self.egui_repaint_requested = true;
    }

    fn request_picker_unicode_icons(&mut self, mut wanted: Vec<char>) {
        let picker = &mut self.chat_emoji_picker;
        if picker.icon_job.is_some() || wanted.is_empty() { return; }
        let Some(font) = picker.catalog.as_ref().and_then(|cat| cat.font.as_ref().cloned()) else { return; };
        wanted.retain(|ch| !picker.icons.contains_key(ch));
        wanted.truncate(64);
        if wanted.is_empty() { return; }
        let Some(pool) = crate::asset_jobs::pool() else { return; };
        let (tx, rx) = mpsc::channel();
        if pool.submit_task(crate::asset_jobs::AssetPriority::Medium, move || {
            let batch = wanted.into_iter().map(|ch| {
                let glyph = crate::ui::rasterize_unicode_glyph(&font, 0, ch).ok().flatten();
                (ch, glyph)
            }).collect();
            let _ = tx.send(batch);
        }) {
            picker.icon_job = Some(rx);
        }
    }

    /// In physical pixels so the button occupies the native composer reserve,
    /// then converted to egui points using the active egui zoom/HiDPI factor.
    fn picker_button_position(&self, ctx: &egui::Context) -> Option<egui::Pos2> {
        let size = self.window.as_ref()?.inner_size();
        let (w, h) = (size.width as f32, size.height as f32);
        let ppp = ctx.pixels_per_point().max(0.01);
        if self.chat_settings.docked() {
            let s = (h / 1080.0).clamp(0.6, 2.0);
            let max_width = w * 0.62;
            let min_width = (280.0 * s).min(max_width);
            let width = ((550.0 * self.hud_layout.chat.extent[0]) * h / 480.0 * 0.92)
                .clamp(min_width, max_width);
            let x0 = 24.0 * s;
            let input_h = 38.0 * s;
            let x = x0 - 4.0 * s + width - 32.0 * s;
            let y = (425.0 / 480.0) * h - input_h + 4.0 * s;
            Some(egui::pos2(x / ppp, y / ppp))
        } else {
            let s = (h / 480.0).max(0.5);
            Some(egui::pos2((w - 42.0 * s) / ppp, 11.0 * s / ppp))
        }
    }

    pub(in crate::app) fn egui_chat_emoji_picker(&mut self, ctx: &egui::Context) {
        if !self.chat_emoji_picker_visible() { return; }
        self.poll_picker_icon_textures(ctx);
        let Some(button_pos) = self.picker_button_position(ctx) else { return; };
        let have_legacy = self.chat_emoji_picker.has_legacy(self.chat_settings.emojis);
        let have_unicode = self.chat_emoji_picker.has_unicode();
        if self.chat_emoji_picker.tab == PickerTab::Legacy && !have_legacy {
            self.chat_emoji_picker.tab = PickerTab::Unicode;
        } else if self.chat_emoji_picker.tab == PickerTab::Unicode && !have_unicode {
            self.chat_emoji_picker.tab = PickerTab::Legacy;
        }
        let mut toggled = false;
        egui::Area::new("chat_emoji_button".into()).order(egui::Order::Foreground)
            .fixed_pos(button_pos).show(ctx, |ui| {
                if ui.add_sized([30.0, 26.0], egui::Button::new("☺"))
                    .on_hover_text("Insert emoji").clicked() { toggled = true; }
            });
        if toggled { self.chat_emoji_picker.open = !self.chat_emoji_picker.open; }
        if !self.chat_emoji_picker.open { return; }
        let viewport = ctx.content_rect();
        let popup_width = 386.0f32.min(viewport.width() - 12.0).max(180.0);
        let popup_height = 378.0f32.min(viewport.height() - 18.0).max(90.0);
        let panel_x = (button_pos.x + 30.0 - popup_width).clamp(6.0, (viewport.width() - popup_width - 6.0).max(6.0));
        let panel_y = if self.chat_settings.docked() {
            (button_pos.y - popup_height - 6.0).max(6.0)
        } else {
            (button_pos.y + 32.0).min((viewport.height() - popup_height - 6.0).max(6.0))
        };
        let mut selection: Option<String> = None;
        let mut wanted_icons = Vec::new();
        // Upload the picker image atlas only once, using precisely the same
        // native pack layout/UVs as the chat renderer.
        if self.chat_emoji_picker.legacy_texture.is_none() {
            if let Some(atlas) = self.chat_emoji_picker.catalog.as_ref().and_then(|cat| cat.legacy.as_ref()) {
                self.chat_emoji_picker.legacy_texture = Some(ctx.load_texture("chat legacy emoji picker atlas",
                    egui::ColorImage::from_rgba_unmultiplied([atlas.width, atlas.height], &atlas.rgba),
                    egui::TextureOptions::LINEAR));
            }
        }
        let picker = &mut self.chat_emoji_picker;
        egui::Area::new("chat_emoji_popup".into()).order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(panel_x, panel_y)).show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(popup_width - 14.0);
                    ui.set_max_height(popup_height);
                    ui.horizontal(|ui| {
                        if have_legacy && have_unicode {
                            ui.selectable_value(&mut picker.tab, PickerTab::Legacy, "Emotes");
                            ui.selectable_value(&mut picker.tab, PickerTab::Unicode, "Unicode");
                        } else if have_legacy { ui.strong("Emotes"); }
                        else { ui.strong("Unicode emoji"); }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("×").clicked() { picker.open = false; }
                        });
                    });
                    ui.add(egui::TextEdit::singleline(&mut picker.search)
                        .hint_text("Search emojis or U+codepoint"));
                    let query = picker.search.trim().to_ascii_lowercase();
                    let cols = ((popup_width - 30.0) / 43.0).floor().max(1.0) as usize;
                    let height = (popup_height - 82.0).max(70.0);
                    if picker.tab == PickerTab::Legacy {
                        if let (Some(atlas), Some(texture)) = (
                            picker.catalog.as_ref().and_then(|cat| cat.legacy.as_ref()),
                            picker.legacy_texture.as_ref(),
                        ) {
                            let indices: Vec<_> = atlas.cells.iter().enumerate().filter_map(|(i, cell)|
                                (query.is_empty() || cell.trigger.to_ascii_lowercase().contains(query.as_str())).then_some(i)
                            ).collect();
                            egui::ScrollArea::vertical().id_salt("chat_legacy_picker_grid")
                                .max_height(height).show_rows(ui, 42.0, indices.len().div_ceil(cols), |ui, rows| {
                                    for row in rows {
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 2.0;
                                            for index in row * cols..((row + 1) * cols).min(indices.len()) {
                                                let cell = &atlas.cells[indices[index]];
                                                let uv = egui::Rect::from_min_max(
                                                    egui::pos2(cell.uv.0[0], cell.uv.0[1]),
                                                    egui::pos2(cell.uv.1[0], cell.uv.1[1]));
                                                let image = egui::Image::new((texture.id(), egui::vec2(30.0, 30.0))).uv(uv);
                                                if ui.add_sized([39.0, 39.0], egui::Button::image(image))
                                                    .on_hover_text(&cell.trigger).clicked() {
                                                    selection = Some(cell.trigger.clone());
                                                }
                                            }
                                        });
                                    }
                                });
                        }
                    } else if let Some(catalog) = picker.catalog.as_ref() {
                        let indices: Vec<_> = catalog.unicode.iter().enumerate().filter_map(|(i, &ch)| {
                            let name = unicode_name(ch);
                            let hex = format!("{:x}", ch as u32);
                            (query.is_empty() || ch.to_string().contains(query.as_str())
                                || name.to_ascii_lowercase().contains(query.as_str())
                                || hex.contains(query.trim_start_matches("u+"))).then_some(i)
                        }).collect();
                        egui::ScrollArea::vertical().id_salt("chat_unicode_picker_grid")
                            .max_height(height).show_rows(ui, 42.0, indices.len().div_ceil(cols), |ui, rows| {
                                for row in rows {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 2.0;
                                        for index in row * cols..((row + 1) * cols).min(indices.len()) {
                                            let ch = catalog.unicode[indices[index]];
                                            let tooltip = format!("{}  U+{:04X}", unicode_name(ch), ch as u32);
                                            let clicked = if let Some(Some(texture)) = picker.icons.get(&ch) {
                                                ui.add_sized([39.0, 39.0], egui::Button::image(egui::Image::new(
                                                    (texture.id(), egui::vec2(30.0, 30.0)))))
                                                    .on_hover_text(&tooltip).clicked()
                                            } else {
                                                if !picker.icons.contains_key(&ch) { wanted_icons.push(ch); }
                                                ui.add_sized([39.0, 39.0], egui::Button::new(ch.to_string()))
                                                    .on_hover_text(&tooltip).clicked()
                                            };
                                            if clicked { selection = Some(ch.to_string()); }
                                        }
                                    });
                                }
                            });
                    }
                });
            });
        if let Some(value) = selection {
            self.insert_picked_chat_emoji(&value);
            self.chat_emoji_picker.open = false;
        }
        if !self.chat_emoji_picker.open {
            ctx.memory_mut(|memory| if let Some(focused) = memory.focused() {
                memory.surrender_focus(focused);
            });
        }
        self.request_picker_unicode_icons(wanted_icons);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_picker_never_claims_an_available_tab() {
        let picker = ChatEmojiPicker::default();
        assert!(!picker.can_show(true));
        assert!(!picker.can_show(false));
    }
}
