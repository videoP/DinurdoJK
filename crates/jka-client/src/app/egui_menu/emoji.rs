//! Color bitmap fallback for the floating console. Egui's font atlas only
//! rasterizes outlines, so retain its layout/hit testing and replace emoji ink.
use std::{collections::HashMap, path::Path, sync::{Arc, Mutex, mpsc}};

type Batch = Vec<(char, Option<crate::ui::UnicodeGlyphBitmap>)>;

#[derive(Default)]
pub(in crate::app) struct EmojiFont {
    // Session lifetime, at most 512 entries, each at most 192x192 RGBA.
    glyphs: HashMap<char, Option<egui::TextureHandle>>,
    files: Arc<Mutex<(bool, Option<Vec<u8>>)>>,
    in_flight: Option<mpsc::Receiver<Batch>>,
}

impl EmojiFont {
    pub(in crate::app) fn poll(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.in_flight else { return };
        match rx.try_recv() {
            Ok(batch) => {
                self.in_flight = None;
                for (ch, bitmap) in batch {
                    let texture = bitmap.filter(|b| !b.rgba.is_empty()).map(|b| {
                        ctx.load_texture(format!("console emoji U+{:X}", ch as u32),
                            egui::ColorImage::from_rgba_unmultiplied(
                                [b.glyph.width as usize, b.glyph.height as usize], &b.rgba),
                            egui::TextureOptions::LINEAR)
                    });
                    self.glyphs.insert(ch, texture);
                }
                ctx.request_repaint();
            }
            Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(50)),
            Err(mpsc::TryRecvError::Disconnected) => self.in_flight = None,
        }
    }

    pub(in crate::app) fn paint(
        &mut self, painter: &egui::Painter, origin: egui::Pos2,
        galley: Arc<egui::Galley>, color: egui::Color32,
        base: &Path, game: Option<&Path>,
    ) {
        let mut wanted = Vec::new();
        let mut replacements = Vec::new();
        for (row_index, row) in galley.rows.iter().enumerate() {
            for (index, glyph) in row.glyphs.iter().enumerate() {
                if !crate::ui::is_emoji_candidate(glyph.chr) { continue; }
                match self.glyphs.get(&glyph.chr) {
                    Some(Some(texture)) => {
                        let cell = glyph.logical_rect().translate(origin.to_vec2() + row.pos.to_vec2());
                        let aspect = texture.size_vec2().x / texture.size_vec2().y.max(1.0);
                        let height = cell.height().min(cell.width() / aspect).max(1.0);
                        let rect = egui::Rect::from_center_size(cell.center(), egui::vec2(height * aspect, height));
                        replacements.push((row_index, index, texture.id(), rect));
                    }
                    None if wanted.len() < 128 && !wanted.contains(&glyph.chr) => wanted.push(glyph.chr),
                    _ => {}
                }
            }
        }
        if replacements.is_empty() {
            painter.galley(origin, galley, color);
        } else {
            // Clone only rows whose ink changes. Cached layout and caret offsets
            // remain untouched; hidden placeholder vertices retain their UVs.
            let mut painted = (*galley).clone();
            for &(row_index, index, _, _) in &replacements {
                let row = Arc::make_mut(&mut painted.rows[row_index].row);
                let start = row.glyphs[index].first_vertex as usize;
                let end = row.glyphs.get(index + 1).map_or(row.visuals.glyph_vertex_range.end,
                    |next| next.first_vertex as usize);
                if let Some(vertices) = row.visuals.mesh.vertices.get_mut(start..end) {
                    for vertex in vertices { vertex.color = egui::Color32::TRANSPARENT; }
                }
            }
            painter.galley(origin, Arc::new(painted), color);
            for (_, _, texture, rect) in replacements {
                painter.image(texture, rect,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
            }
        }
        if self.in_flight.is_some() || wanted.is_empty() || self.glyphs.len() >= 512 { return; }
        wanted.truncate(512 - self.glyphs.len());
        let Some(pool) = crate::asset_jobs::pool() else { return };
        let files = Arc::clone(&self.files);
        let base = base.to_owned();
        let game = game.map(Path::to_owned);
        let (tx, rx) = mpsc::channel();
        if pool.submit_task(crate::asset_jobs::AssetPriority::High, move || {
            let Ok(mut files) = files.lock() else { return };
            if !files.0 {
                files.1 = crate::ui::load_unicode_font("NotoColorEmoji.ttf", &base, game.as_deref());
                files.0 = true;
            }
            let batch = wanted.into_iter().map(|ch| {
                let bitmap = files.1.as_deref().and_then(|bytes|
                    crate::ui::rasterize_unicode_glyph(bytes, 0, ch).ok().flatten());
                (ch, bitmap)
            }).collect();
            let _ = tx.send(batch);
        }) {
            self.in_flight = Some(rx);
            painter.ctx().request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}
