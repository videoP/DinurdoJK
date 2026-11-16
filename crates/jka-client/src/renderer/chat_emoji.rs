//! TaystJK gfx/emoji emotes: asynchronous VFS discovery/PNG decoding and a
//! single native-size RGBA atlas. Never access assets or pack images per frame.
use super::{overlay, upload_texture, Renderer, TextureData};
use crate::ui::{ChatEmojiAtlas, ChatEmojiCell, tayst_emoji_trigger};
use jka_assets::pk3::AssetSearchPath;
use std::{path::PathBuf, sync::{mpsc, Arc}};

pub(super) struct ChatEmojiBuild {
    texture: TextureData,
    atlas: ChatEmojiAtlas,
}

const MAX_EMOTES: usize = 512;

fn load_emotes(base: &std::path::Path, game: Option<&std::path::Path>, jk2: Option<u8>, limit: u32) -> ChatEmojiBuild {
    let mut assets = match AssetSearchPath::open_game_as(base, game, jk2) {
        Ok(assets) => assets,
        Err(error) => {
            eprintln!("Chat emoji assets: {error}");
            let (texture, _) = overlay::assemble_native_icon_atlas(Vec::new(), limit);
            return ChatEmojiBuild { texture, atlas: ChatEmojiAtlas::default() };
        }
    };
    // AssetSearchPath performs the same mounted PK3/loose-file precedence as
    // the rest of JKA. Keep the actual filename for reading, never rewrite
    // escaped punctuation or upper-case its PK3 qpath.
    let mut entries: Vec<_> = assets.names().filter_map(|path| {
        let filename = path.strip_prefix("gfx/emoji/")?;
        let trigger = tayst_emoji_trigger(filename)?;
        Some((path.to_owned(), trigger))
    }).collect();
    entries.sort();
    entries.truncate(MAX_EMOTES);
    let mut names = Vec::new();
    let mut images = Vec::new();
    for (path, trigger) in entries {
        // Reject invalid PNGs without registering a blank emote. The public
        // decoder enforces the existing UI image-size and decode policies.
        let Some(texture) = super::try_load_texture_asset_from_search_path(
            &mut assets, &path, true, false, true, "chat emoji",
        ) else { continue; };
        let Some(image) = image::RgbaImage::from_raw(texture.width, texture.height, texture.rgba.into_vec()) else {
            eprintln!("Chat emoji {path}: invalid RGBA size");
            continue;
        };
        if image.width() == 0 || image.height() == 0 { continue; }
        names.push(trigger);
        images.push(Some(image));
    }
    let (mut texture, uvs) = overlay::assemble_native_icon_atlas(images, limit);
    texture.label = "JKA TaystJK named chat emoji atlas".into();
    let atlas = ChatEmojiAtlas::from_cells(names.into_iter().zip(uvs).map(|(trigger, uv)|
        ChatEmojiCell { trigger, uv }));
    ChatEmojiBuild { texture, atlas }
}

/// A CPU copy of the exact same packed legacy atlas/shortcodes for the egui
/// picker. The picker requests it only while chat is open; the actual chat
/// renderer continues to own its own WGPU atlas and asynchronous lifetime.
pub(crate) struct ChatPickerLegacyAtlas {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) rgba: Vec<u8>,
    pub(crate) cells: Vec<ChatEmojiCell>,
}

pub(crate) fn load_chat_picker_atlas(
    base: &std::path::Path, game: Option<&std::path::Path>, jk2: Option<u8>,
) -> Option<ChatPickerLegacyAtlas> {
    let loaded = load_emotes(base, game, jk2, 4096);
    if loaded.atlas.count == 0 { return None; }
    Some(ChatPickerLegacyAtlas {
        width: loaded.texture.width as usize,
        height: loaded.texture.height as usize,
        rgba: loaded.texture.rgba.into_vec(),
        cells: loaded.atlas.sorted_cells(),
    })
}

impl Renderer {
    /// Returns true if the active atlas changed and chat geometry must be rebuilt.
    pub(in crate::renderer) fn sync_ui_chat_emojis(&mut self) -> bool {
        let scope = (
            self.detail_texture_game.clone(),
            jka_assets::pk3::configured_filesystem_roots(&self.base).active_jk2,
        );
        if self.ui_chat_emoji_scope.as_ref() != Some(&scope) {
            let previous_scope = self.ui_chat_emoji_scope.replace(scope.clone());
            self.ui_chat_emoji_job = None; // discard old game's in-flight response
            self.ui_chat_emoji_loaded = false;
            self.ui_state.chat_emoji_atlas = None;
            if previous_scope.is_some() {
                self._ui_chat_emojis = upload_texture(&self.device, &self.queue,
                    &super::empty_ui_font_texture("JKA chat emoji atlas (not loaded)"));
                self.rebuild_ui_bind_group();
            }
        }
        if !self.ui_state.chat_settings.emojis { return false; }
        if let Some(job) = self.ui_chat_emoji_job.as_ref() {
            match job.try_recv() {
                Ok(result) => {
                    self.ui_chat_emoji_job = None;
                    self._ui_chat_emojis = upload_texture(&self.device, &self.queue, &result.texture);
                    let count = result.atlas.count;
                    self.ui_state.chat_emoji_atlas = (count > 0).then(|| Arc::new(result.atlas));
                    self.ui_chat_emoji_loaded = true;
                    self.rebuild_ui_bind_group();
                    rverbose!(1, "TaystJK chat emotes: {count} loaded in one GPU atlas");
                    return true;
                }
                Err(mpsc::TryRecvError::Empty) => return false,
                Err(mpsc::TryRecvError::Disconnected) => self.ui_chat_emoji_job = None,
            }
        }
        if self.ui_chat_emoji_loaded { return false; }
        let Some(pool) = crate::asset_jobs::pool() else { return false; };
        let (sender, receiver) = mpsc::channel();
        let base = self.base.clone();
        let game: Option<PathBuf> = scope.0;
        let jk2 = scope.1;
        let max_texture_size = self.device.limits().max_texture_dimension_2d;
        if pool.submit_task(crate::asset_jobs::AssetPriority::Medium, move || {
            let result = load_emotes(&base, game.as_deref(), jk2, max_texture_size);
            let _ = sender.send(result);
        }) {
            self.ui_chat_emoji_job = Some(receiver);
        }
        false
    }
}

impl Renderer {
    /// Called once after fs_restart. Renderer state, not the app's UiSnapshot,
    /// owns the GPU atlas and in-flight job lifetime.
    pub(in crate::renderer) fn reload_ui_chat_emojis(&mut self) {
        self.ui_chat_emoji_job = None;
        self.ui_chat_emoji_scope = None;
        self.ui_chat_emoji_loaded = false;
        self.ui_state.chat_emoji_atlas = None;
        self._ui_chat_emojis = upload_texture(&self.device, &self.queue,
            &super::empty_ui_font_texture("JKA chat emoji atlas (refreshing)"));
        self.rebuild_ui_bind_group();
        self.sync_ui_chat_emojis();
        self.rebuild_transient_ui();
    }
}
