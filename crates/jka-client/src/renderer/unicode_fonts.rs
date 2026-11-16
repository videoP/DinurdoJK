//! Optional, lazy Unicode glyphs for the existing UI fallback texture slot.
//!
//! No font files are opened and no large GPU texture is allocated at startup.
//! UI builds only collect codepoints that could not use their primary font.
//! One existing asset worker loads/caches the needed packaged fonts and
//! rasterizes requested glyphs. The render thread only places completed small
//! rectangles into the atlas, then rebuilds the existing UI geometry.
use super::{ui, GpuImage, Renderer};
use jka_assets::pk3::AssetSearchPath;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc::{self, Receiver}},
};

const ATLAS_SIDE: u32 = 2048;
const MAX_BATCH: usize = 128;
const FONT_FILE_LIMIT: usize = 32 * 1024 * 1024;

struct FontJobResult {
    glyphs: Vec<(char, Option<ui::UnicodeGlyphBitmap>)>,
}

#[derive(Default)]
struct FontFileCache {
    // Failed font reads are remembered as well as successful ones; missing
    // optional files cannot trigger disk access once per character/frame.
    files: HashMap<&'static str, Option<Vec<u8>>>,
}

pub(super) struct UnicodeFontWorker {
    pub requests: Arc<Mutex<HashSet<char>>>,
    in_flight: Option<Receiver<FontJobResult>>,
    files: Arc<Mutex<FontFileCache>>,
    base: PathBuf,
    game: Option<PathBuf>,
    missing: HashSet<char>,
    glyphs: HashMap<char, ui::ProportionalGlyph>,
    cap_heights: HashMap<char, f32>,
    x: u32,
    y: u32,
    row_height: u32,
    resident: bool,
    warned_atlas_full: bool,
}

impl UnicodeFontWorker {
    pub(super) fn new(base: &Path, game: Option<&Path>) -> Self {
        Self {
            requests: Arc::new(Mutex::new(HashSet::new())),
            in_flight: None, files: Arc::new(Mutex::new(FontFileCache::default())),
            base: base.to_owned(), game: game.map(Path::to_owned),
            missing: HashSet::new(), glyphs: HashMap::new(), cap_heights: HashMap::new(),
            x: 0, y: 0, row_height: 0, resident: false, warned_atlas_full: false,
        }
    }

    fn allocate(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        let width = (width + 3).div_ceil(8) * 8;
        let height = (height + 3).div_ceil(8) * 8;
        if width > ATLAS_SIDE || height > ATLAS_SIDE { return None; }
        if self.x + width > ATLAS_SIDE {
            self.x = 0;
            self.y += self.row_height;
            self.row_height = 0;
        }
        if self.y + height > ATLAS_SIDE { return None; }
        let origin = (self.x, self.y);
        self.x += width;
        self.row_height = self.row_height.max(height);
        Some(origin)
    }
}

/// The choice is a *script hint*, not a declaration of language. Han codepoints
/// alone cannot distinguish Chinese from Japanese; default Han to SC and
/// route kana/Hangul to JP/KR faces respectively.
fn preferred_font(ch: char) -> (&'static str, u32) {
    let cp = ch as u32;
    match cp {
        0x3000..=0x30FF | 0x31F0..=0x31FF => ("NotoSansCJK-Regular.ttc", 0), // Japanese
        0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F
        | 0xAC00..=0xD7AF | 0xD7B0..=0xD7FF => ("NotoSansCJK-Regular.ttc", 1), // Korean
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF
        | 0x20000..=0x323AF => ("NotoSansCJK-Regular.ttc", 2), // Han default SC
        0x0530..=0x058F => ("NotoSansArmenian-Regular.ttf", 0),
        0x0590..=0x05FF | 0xFB1D..=0xFB4F => ("NotoSansHebrew-Regular.ttf", 0),
        0x0600..=0x06FF | 0x08A0..=0x08FF | 0xFB50..=0xFDFF
        | 0xFE70..=0xFEFF => ("NotoNaskhArabic-Regular.ttf", 0),
        0x0750..=0x077F => ("NotoNastaliqUrdu-Regular.ttf", 0),
        0x0780..=0x07BF => ("NotoSansThaana-Regular.ttf", 0),
        0x07C0..=0x07FF => ("NotoSansNKo-Regular.ttf", 0),
        0x0700..=0x074F => ("NotoSansSyriac-Regular.ttf", 0),
        0x0900..=0x097F | 0xA8E0..=0xA8FF => ("NotoSansDevanagariUI-Regular.ttf", 0),
        0x0980..=0x09FF => ("NotoSansBengaliUI-Regular.ttf", 0),
        0x0A00..=0x0A7F => ("NotoSansGurmukhiUI-Regular.ttf", 0),
        0x0A80..=0x0AFF => ("NotoSansGujaratiUI-Regular.ttf", 0),
        0x0B00..=0x0B7F => ("NotoSansOriyaUI-Regular.ttf", 0),
        0x0B80..=0x0BFF => ("NotoSansTamilUI-Regular.ttf", 0),
        0x0C00..=0x0C7F => ("NotoSansTeluguUI-Regular.ttf", 0),
        0x0C80..=0x0CFF => ("NotoSansKannadaUI-Regular.ttf", 0),
        0x0D00..=0x0D7F => ("NotoSansMalayalamUI-Regular.ttf", 0),
        0x0D80..=0x0DFF => ("NotoSansSinhalaUI-Regular.ttf", 0),
        0x0E00..=0x0E7F => ("NotoSansThaiUI-Regular.ttf", 0),
        0x0E80..=0x0EFF => ("NotoSansLaoUI-Regular.ttf", 0),
        0x0F00..=0x0FFF => ("NotoSerifTibetan-Regular.ttf", 0),
        0x1000..=0x109F | 0xA9E0..=0xA9FF | 0xAA60..=0xAA7F => ("NotoSansMyanmarUI-Regular.ttf", 0),
        0x10A0..=0x10FF | 0x1C90..=0x1CBF | 0x2D00..=0x2D2F => ("NotoSansGeorgian-Regular.ttf", 0),
        0x1200..=0x139F | 0x2D80..=0x2DDF | 0xAB00..=0xAB2F => ("NotoSansEthiopic-Regular.ttf", 0),
        0x13A0..=0x13FF | 0xAB70..=0xABBF => ("NotoSansCherokee-Regular.ttf", 0),
        0x1400..=0x167F | 0x18B0..=0x18FF => ("NotoSansCanadianAboriginal-Regular.ttf", 0),
        0x1800..=0x18AF | 0x11660..=0x1167F => ("NotoSansMongolian-Regular.ttf", 0),
        0x1B00..=0x1B7F => ("NotoSansBalinese-Regular.ttf", 0),
        0x1780..=0x17FF | 0x19E0..=0x19FF => ("NotoSansKhmerUI-Regular.ttf", 0),
        0xAA80..=0xAADF => ("NotoSansTaiViet-Regular.ttf", 0),
        0xA980..=0xA9DF => ("NotoSansJavanese-Regular.ttf", 0),
        0x2D30..=0x2D7F => ("NotoSansTifinagh-Regular.ttf", 0),
        0xA500..=0xA63F => ("NotoSansVai-Regular.ttf", 0),
        0xA000..=0xA48F => ("NotoSansYi-Regular.ttf", 0),
        0xABC0..=0xABFF => ("NotoSansMeeteiMayek-Regular.ttf", 0),
        0x1C50..=0x1C7F => ("NotoSansOlChiki-Regular.ttf", 0),
        0x1E900..=0x1E95F => ("NotoSansAdlam-Regular.ttf", 0),
        // Jura is only considered on a missing primary glyph. This includes
        // Latin Extended, Cyrillic, Greek, and explicit Jura chat styling.
        _ => ("Jura-Regular.ttf", 0),
    }
}

fn load_font<'a>(
    name: &'static str,
    files: &'a mut FontFileCache,
    base: &Path,
    game: Option<&Path>,
) -> Option<&'a [u8]> {
    if !files.files.contains_key(name) {
        let loose = base.parent().unwrap_or(base).join("DinurdoJK").join("fonts").join(name);
        let bytes = std::fs::metadata(&loose).ok()
            .filter(|metadata| metadata.len() <= FONT_FILE_LIMIT as u64)
            .and_then(|_| std::fs::read(&loose).ok())
            .or_else(|| {
                // Shared dinurdo-assets.pk4 and normal game filesystem paths.
                let mut assets = AssetSearchPath::open_game(base, game).ok()?;
                Some(assets.read(&format!("fonts/{name}"), FONT_FILE_LIMIT).ok()??.bytes)
            })
            .filter(|bytes: &Vec<u8>| bytes.len() <= FONT_FILE_LIMIT);
        if bytes.is_none() {
            eprintln!("Unicode font not installed: {name} (fallback remains available)");
        }
        files.files.insert(name, bytes);
    }
    files.files.get(name).and_then(Option::as_deref)
}

fn raster_one(
    ch: char, files: &mut FontFileCache, base: &Path, game: Option<&Path>,
) -> Option<ui::UnicodeGlyphBitmap> {
    let primary = preferred_font(ch);
    let attempts = [
        primary,
        ("Jura-Regular.ttf", 0), ("Jura-Bold.ttf", 0),
        ("NotoSans-Regular.ttf", 0),
        ("NotoSansSymbols-Regular.ttf", 0),
        ("NotoSansSymbols2-Regular.ttf", 0),
    ];
    let mut tried = HashSet::new();
    for (name, face) in attempts {
        if !tried.insert((name, face)) { continue; }
        let Some(bytes) = load_font(name, files, base, game) else { continue; };
        match ui::rasterize_unicode_glyph(bytes, face, ch) {
            Ok(Some(glyph)) => return Some(glyph),
            Ok(None) => {}
            Err(error) => {
                // Malformed fonts aren't retried for this character.
                eprintln!("Unicode font {name} face {face}: {error}");
            }
        }
    }
    None
}

impl Renderer {
    /// Runs at the UI/status cadence (~90ms), not at multiplayer frame rate.
    pub(in crate::renderer) fn poll_unicode_fonts(&mut self) {
        if let Some(results) = self.ui_unicode_worker.in_flight.as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.ui_unicode_worker.in_flight = None;
            let mut changed = false;
            let mut failures_updated = false;
            let mut needs_binding = false;
            for (ch, result) in results.glyphs {
                let Some(bitmap) = result else {
                    failures_updated |= self.ui_unicode_worker.missing.insert(ch);
                    continue;
                };
                let mut glyph = bitmap.glyph;
                let (width, height) = (glyph.width.max(0) as u32, glyph.height.max(0) as u32);
                if width > 0 && height > 0 {
                    let Some((x, y)) = self.ui_unicode_worker.allocate(width, height) else {
                        // Bounded memory. Never crash or overwrite the glyphs
                        // referenced by already-built/retained vertex buffers.
                        if !self.ui_unicode_worker.warned_atlas_full {
                            eprintln!("Unicode atlas full; further uncached glyphs use ? until renderer restart");
                            self.ui_unicode_worker.warned_atlas_full = true;
                        }
                        failures_updated |= self.ui_unicode_worker.missing.insert(ch);
                        continue;
                    };
                    if !self.ui_unicode_worker.resident {
                        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                            label: Some("lazy JKA/Jura/Noto Unicode glyph atlas"),
                            size: wgpu::Extent3d { width: ATLAS_SIDE, height: ATLAS_SIDE, depth_or_array_layers: 1 },
                            mip_level_count: 1, sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format: wgpu::TextureFormat::Rgba8Unorm,
                            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                            view_formats: &[],
                        });
                        let view = texture.create_view(&Default::default());
                        self._ui_jura_font = GpuImage::new(texture, view);
                        self.ui_unicode_worker.resident = true;
                        needs_binding = true;
                    }
                    let rgba: Vec<u8> = bitmap.coverage.iter()
                        .flat_map(|alpha| [255u8, 255, 255, *alpha]).collect();
                    self.queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &self._ui_jura_font._texture, mip_level: 0,
                            origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All,
                        },
                        &rgba,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height),
                        },
                        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                    );
                    glyph.s = x as f32 / ATLAS_SIDE as f32;
                    glyph.t = y as f32 / ATLAS_SIDE as f32;
                    glyph.s2 = (x + width) as f32 / ATLAS_SIDE as f32;
                    glyph.t2 = (y + height) as f32 / ATLAS_SIDE as f32;
                }
                self.ui_unicode_worker.cap_heights.insert(ch, bitmap.cap_height);
                self.ui_unicode_worker.glyphs.insert(ch, glyph);
                changed = true;
            }
            if changed || failures_updated {
                self.ui_jura_metrics = Some(Arc::new(ui::UnicodeFallbackFont {
                    glyphs: self.ui_unicode_worker.glyphs.clone(),
                    cap_heights: self.ui_unicode_worker.cap_heights.clone(),
                    failed: self.ui_unicode_worker.missing.clone(),
                    height: 64, cap_height: 42.0,
                }));
                if needs_binding {
                    self.rebuild_ui_bind_group();
                    // Studio custom standings bind group captures the same
                    // fallback view; regenerate it on the next studio draw.
                    self.studio_race_font = None;
                }
                if changed { self.rebuild_ui(); }
            }
        }
        if self.ui_unicode_worker.in_flight.is_some() { return; }
        let mut requested = match self.ui_unicode_worker.requests.lock() {
            Ok(requested) => requested,
            Err(_) => return,
        };
        let mut all: Vec<_> = requested.drain()
            .filter(|ch| !self.ui_unicode_worker.glyphs.contains_key(ch)
                && !self.ui_unicode_worker.missing.contains(ch))
            .collect();
        all.sort_unstable();
        let excess = all.split_off(all.len().min(MAX_BATCH));
        requested.extend(excess);
        let wanted = all;
        drop(requested);
        if wanted.is_empty() { return; }
        let Some(pool) = crate::asset_jobs::pool() else {
            if let Ok(mut pending) = self.ui_unicode_worker.requests.lock() {
                pending.extend(wanted);
            }
            return;
        };
        let retry = wanted.clone();
        let (tx, rx) = mpsc::channel();
        let files = Arc::clone(&self.ui_unicode_worker.files);
        let base = self.ui_unicode_worker.base.clone();
        let game = self.ui_unicode_worker.game.clone();
        if pool.submit_task(crate::asset_jobs::AssetPriority::High, move || {
            let Ok(mut files) = files.lock() else { return; };
            let glyphs = wanted.into_iter()
                .map(|ch| (ch, raster_one(ch, &mut files, &base, game.as_deref())))
                .collect();
            let _ = tx.send(FontJobResult { glyphs });
        }) {
            self.ui_unicode_worker.in_flight = Some(rx);
        } else if let Ok(mut pending) = self.ui_unicode_worker.requests.lock() {
            // Bounded asset queue can decline jobs during a busy map load.
            // Retry later instead of dropping the first occurrence of a glyph.
            pending.extend(retry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scripts_select_separate_lazy_families() {
        assert_eq!(preferred_font('你'), ("NotoSansCJK-Regular.ttc", 2));
        assert_eq!(preferred_font('あ'), ("NotoSansCJK-Regular.ttc", 0));
        assert_eq!(preferred_font('한'), ("NotoSansCJK-Regular.ttc", 1));
        assert_eq!(preferred_font('م').0, "NotoNaskhArabic-Regular.ttf");
        assert_eq!(preferred_font('अ').0, "NotoSansDevanagariUI-Regular.ttf");
        assert_eq!(preferred_font('Ж').0, "Jura-Regular.ttf");
    }
    #[test]
    fn atlas_is_bounded_and_makes_no_allocation_until_first_glyph() {
        let mut worker = UnicodeFontWorker::new(Path::new("base"), None);
        assert!(!worker.resident);
        assert_eq!(worker.allocate(60, 70), Some((0, 0)));
        assert_eq!(worker.allocate(ATLAS_SIDE + 1, 70), None);
        assert!(!worker.resident);
    }
}
