//! Overlay.
use crate::renderer::{
    append_vignette_vertices, create_menu_backdrop_resources, create_screen_fx_pipeline,
    load_ui_key_atlas, pipeline_hash, try_load_texture_asset,
    try_load_texture_asset_from_search_path, try_load_ui_font_asset, ui, upload_texture, Arc,
    AssetSearchPath, EguiRenderData, FxBlend, GpuImage, HashMap, Instant, Mat4,
    MenuBackdropResources, Path, PathBuf, PipelineJobKey, Renderer, TextureData, UiSnapshot,
    UiVertex, Vec3, SCREEN_FX_BUFFER_BYTES, UI_BUFFER_BYTES, UI_DYNAMIC_BUFFER_BYTES,
    UI_TRANSIENT_BUFFER_BYTES,
};

/// Shader source tags are sent through the existing UiVertex without adding
/// attributes or changing the native UI WGSL pipeline layout.
fn ui_shader_icon_index(source: f32) -> Option<usize> {
    let index = source - ui::ICON_SHADER_SOURCE_BASE;
    if index >= 0.0 && index < ui::ICON_NAMES.len() as f32 && index.fract() == 0.0 {
        Some(index as usize)
    } else { None }
}

/// Unified shader cache key: native icon indices first, then extension slots.
const MAX_UI_HUD_ATLAS_IMAGE_SLOTS: usize = 128; // script APIs remain capped at 32
const USER_SHADER_INDEX_BASE: usize = ui::ICON_NAMES.len() + MAX_UI_HUD_ATLAS_IMAGE_SLOTS;

fn ui_user_shader_draw_index(source: f32) -> Option<usize> {
    let index = source - ui::EXT_USER_SHADER_SOURCE_BASE;
    if index >= 0.0 && index < (64 * crate::extensions::MAX_HUD_COMMANDS) as f32
        && index.fract() == 0.0 { Some(index as usize) } else { None }
}

fn ui_shader_material_index(source: f32) -> Option<usize> {
    if let Some(index) = ui_user_shader_draw_index(source) {
        return Some(USER_SHADER_INDEX_BASE + index);
    }
    if let Some(index) = ui_shader_icon_index(source) { return Some(index); }
    let index = source - ui::EXT_IMAGE_SHADER_SOURCE_BASE;
    if index >= 0.0 && index < MAX_UI_HUD_ATLAS_IMAGE_SLOTS as f32
        && index.fract() == 0.0
    { Some(ui::ICON_NAMES.len() + index as usize) } else { None }
}

const MAX_HUD_SHADED_DRAWS: usize = 128;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(in crate::renderer) struct HudShaderUniform {
    atlas_rect: [f32; 4],
    values: [f32; 4],
    clock: [f32; 4],
}

pub(in crate::renderer) struct HudShaderGpu {
    layout: wgpu::PipelineLayout,
    uniforms: wgpu::Buffer,
    group: wgpu::BindGroup,
    stride: usize,
    pending: Vec<u8>,
    // A shader can draw into either the native sRGB UI view or its encoded
    // UNORM alias (RGBA UNORM for the HDR UI canvas). Pipeline targets must
    // exactly match the active render-pass attachment's format.
    pipelines: HashMap<String, HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>>,
    requested: HashMap<String, HashMap<wgpu::TextureFormat, u64>>,
}

impl HudShaderGpu {
    fn new(device: &wgpu::Device, atlas_layout: &wgpu::BindGroupLayout) -> Self {
        let stride = device.limits().min_uniform_buffer_offset_alignment.max(256) as usize;
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Extension HUD WGSL uniforms"),
            size: (stride * MAX_HUD_SHADED_DRAWS) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Extension HUD WGSL params layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true,
                    min_binding_size: std::num::NonZeroU64::new(std::mem::size_of::<HudShaderUniform>() as u64),
                },
                count: None,
            }],
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Extension HUD WGSL params"), layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniforms, offset: 0,
                    size: std::num::NonZeroU64::new(std::mem::size_of::<HudShaderUniform>() as u64),
                }),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Extension HUD WGSL pipeline layout"),
            bind_group_layouts: &[Some(atlas_layout), Some(&uniform_layout)],
            immediate_size: 0,
        });
        Self { layout, uniforms, group, stride, pending: Vec::new(), pipelines: HashMap::new(), requested: HashMap::new() }
    }

    fn append(&mut self, uniform: HudShaderUniform) -> Option<u32> {
        let slot = self.pending.len() / self.stride;
        if slot >= MAX_HUD_SHADED_DRAWS { return None; }
        self.pending.extend_from_slice(bytemuck::bytes_of(&uniform));
        self.pending.resize((slot + 1) * self.stride, 0);
        Some((slot * self.stride) as u32)
    }
}

pub(in crate::renderer) struct PendingUiLevelshot {
    pub(in crate::renderer) key: String,
    pub(in crate::renderer) map_name: String,
    pub(in crate::renderer) game: Option<PathBuf>,
}

pub(in crate::renderer) struct ScreenFxTexture {
    pub(in crate::renderer) _image: GpuImage,
    pub(in crate::renderer) bind_group: wgpu::BindGroup,
    pub(in crate::renderer) encoded_bind_group: wgpu::BindGroup,
}

/// Keep authored 2D stages and implicit atlas materials in the same encoded
/// sampling domain. The alternate view aliases the original texture allocation.
fn encoded_image_view(image: &GpuImage) -> wgpu::TextureView {
    if image._texture.format() == wgpu::TextureFormat::Rgba8UnormSrgb {
        image._texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rgba8Unorm),
            ..Default::default()
        })
    } else {
        image.view.clone()
    }
}

pub(in crate::renderer) fn create_ui_texture_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    images: [&GpuImage; 9],
    encoded: bool,
) -> wgpu::BindGroup {
    let views: Vec<_> = images.iter().map(|image| {
        if encoded { encoded_image_view(image) } else { image.view.clone() }
    }).collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(if encoded { "JKA encoded UI textures" } else { "JKA UI textures" }),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&views[0]) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&views[1]) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&views[2]) },
            wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&views[3]) },
            wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&views[4]) },
            wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(&views[5]) },
            wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::TextureView(&views[6]) },
            wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::TextureView(&views[7]) },
            wgpu::BindGroupEntry { binding: 9, resource: wgpu::BindingResource::TextureView(&views[8]) },
        ],
    })
}

/// Rhai HUD image jobs use the existing priority-based asset pool, never a
/// dedicated thread. Both caches are scoped to this renderer/game mount and
/// are dropped when the extension HUD or active game is reset.
pub(in crate::renderer) struct UiExtImageLoader {
    base: PathBuf,
    game: Option<PathBuf>,
    results_tx: std::sync::mpsc::Sender<UiExtImageResult>,
    results: std::sync::mpsc::Receiver<UiExtImageResult>,
    cache: Arc<std::sync::Mutex<UiExtImageCache>>,
    pending: bool,
}

#[derive(Default)]
struct UiExtImageCache {
    parsed: bool,
    shaders: Option<std::collections::BTreeMap<String, jka_assets::shader::Shader>>,
    /// Previously decoded authored stages: do not reparse/redecode an icon.
    stages: HashMap<String, Vec<crate::fx::draw::FxMaterial>>,
}

struct UiExtImageResult {
    wanted: Vec<String>,
    first_new: usize,
    pixels: Vec<Option<Vec<u8>>>,
    source_dimensions: Vec<Option<[u32; 2]>>,
    shader_flags: Vec<bool>,
    shader_stages: Vec<(usize, Vec<crate::fx::draw::FxMaterial>)>,
}

impl UiExtImageLoader {
    fn start(base: PathBuf, game: Option<PathBuf>) -> Self {
        let (results_tx, results) = std::sync::mpsc::channel();
        Self {
            base, game, results_tx, results,
            cache: Arc::new(std::sync::Mutex::new(UiExtImageCache::default())),
            pending: false,
        }
    }

    /// Submit CPU-only work without waiting. If the queue is full, leave the
    /// request unmarked so the renderer can retry on its next UI update.
    fn enqueue(&mut self, wanted: Vec<String>, first_new: usize) -> bool {
        let Some(pool) = crate::asset_jobs::pool() else { return false; };
        let cache = Arc::downgrade(&self.cache);
        let base = self.base.clone();
        let game = self.game.clone();
        let complete = self.results_tx.clone();
        let queued = pool.submit_task(crate::asset_jobs::AssetPriority::Medium, move || {
            // A HUD unload or game switch invalidates its cache and cancels
            // queued work before any costly shader-library parsing begins.
            let Some(cache) = cache.upgrade() else { return; };
            if first_new > wanted.len() { return; }
            let mut assets = match AssetSearchPath::open_game(&base, game.as_deref()) {
                Ok(assets) => assets,
                Err(err) => {
                    eprintln!("Extension HUD image asset search: {err}");
                    let count = wanted.len().saturating_sub(first_new);
                    let _ = complete.send(UiExtImageResult {
                        wanted, first_new,
                        pixels: vec![None; count],
                        source_dimensions: vec![None; count],
                        shader_flags: vec![false; count],
                        shader_stages: Vec::new(),
                    });
                    return;
                }
            };
            let mut cache = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if !cache.parsed {
                let started = Instant::now();
                let mut warnings = Vec::new();
                cache.shaders = match crate::materials::shader_library(
                    &mut assets, &mut warnings, false,
                ) {
                    Ok((library, _)) => Some(library),
                    Err(err) => { eprintln!("Extension HUD shader library: {err}"); None }
                };
                cache.parsed = true;
                rverbose!(1, "UI extension shader library parsed on asset worker in {:.1} ms",
                    started.elapsed().as_secs_f64() * 1000.0);
                for warning in warnings {
                    rverbose!(2, "Extension HUD shader library: {warning}");
                }
            }
            let (pixels, shader_flags, source_dimensions) = decode_ui_extension_image_cells(
                &mut assets, cache.shaders.as_ref(), &wanted[first_new..],
            );
            let mut shader_stages = Vec::new();
            let mut stage_resolver: Option<crate::cgame::entity_presenter::EntityPresenter> = None;
            for (offset, &authored) in shader_flags.iter().enumerate() {
                if !authored { continue; }
                let name = &wanted[first_new + offset];
                if !cache.stages.contains_key(name) {
                    if stage_resolver.is_none() {
                        stage_resolver = AssetSearchPath::open_game(&base, game.as_deref())
                            .ok().and_then(|assets|
                                crate::cgame::entity_presenter::EntityPresenter::new(assets, false).ok());
                    }
                    let stages = stage_resolver.as_mut()
                        .and_then(|resolver| resolver.fx_material_stages_if_present(name))
                        .unwrap_or_default();
                    cache.stages.insert(name.clone(), stages);
                }
                shader_stages.push((
                    ui::ICON_NAMES.len() + first_new + offset,
                    cache.stages.get(name).cloned().unwrap_or_default(),
                ));
            }
            drop(cache);
            let _ = complete.send(UiExtImageResult {
                wanted, first_new, pixels, source_dimensions, shader_flags, shader_stages,
            });
        });
        if queued { self.pending = true; }
        queued
    }
}

impl Renderer {
    pub(in crate::renderer) fn set_ui(&mut self, mut ui_state: UiSnapshot) {
        let needs_key_atlas = !self.ui_keys_loaded && ui_state.movement_keys.mode != 0;
        // The app is not the owner of pipeline jobs; preserve the render-thread
        // status when a gameplay/menu UI snapshot arrives between samples.
        // The app never owns GPU atlas placement. Do not overwrite the UV
        // lookup after a scroll/HUD snapshot arrives on the render thread.
        ui_state.icon_atlas_uvs.clone_from(&self.ui_state.icon_atlas_uvs);
        ui_state.icon_shader_flags.clone_from(&self.ui_state.icon_shader_flags);
        ui_state.extension_shader_flags.clone_from(&self.ui_state.extension_shader_flags);
        ui_state.extension_image_dimensions.clone_from(&self.ui_state.extension_image_dimensions);
        // Likewise the clean font: loaded once by the renderer, never by the app.
        ui_state.clean_font.clone_from(&self.ui_clean_font_metrics);
        ui_state.avatar_atlas.clone_from(&self.ui_avatar_atlas);
        ui_state.shader_compile_progress = self.ui_state.shader_compile_progress;
        ui_state.shader_compile_stage = self.ui_state.shader_compile_stage.clone();
        self.ui_state = ui_state;
        if needs_key_atlas {
            self.load_ui_key_atlas_now();
        }
        self.load_ui_icons_if_needed();
        self.sync_ui_team_icons();
        self.sync_enemy_icon_shader();
        self.sync_ui_extension_images();
        self.sync_ui_loading_background();
        self.rebuild_ui();
    }

    /// Set rapidly changing scope/binocular/rocket state independently of
    /// SetUi and SetTransientUi. Vertices are late-latched at render time.
    pub(in crate::renderer) fn set_combat_ui(
        &mut self,
        zoom_scope: Option<ui::UiZoomScope>,
        rocket_lock: Option<ui::UiRocketLock>,
    ) {
        // The static crosshair is cached in the transient HUD buffer, whereas
        // zoomMode changes arrive through this lightweight per-tick command.
        // Rebuild that buffer on scope entry/exit only, not on every zoom frame.
        let zoom_was_active = self.ui_state.zoom_scope.is_some();
        self.ui_state.zoom_scope = zoom_scope;
        self.ui_state.rocket_lock = rocket_lock;
        self.load_ui_icons_if_needed();
        if zoom_was_active != self.ui_state.zoom_scope.is_some()
            && self.ui_state.crosshair.scope_mode != 1
        {
            self.rebuild_transient_ui();
        }
    }

    pub(in crate::renderer) fn set_extension_hud(
        &mut self,
        widgets: Vec<crate::extensions::PlacedHudWidget>,
        hidden: crate::extensions::NativeHudMask,
        images: Vec<String>,
        shaders: Vec<(String, String)>,
    ) {
        if self.hud_shader_sources != shaders {
            self.hud_shader_sources = shaders;
            if self.hud_shader_sources.is_empty() {
                // Disabling extensions releases their shader GPU state.
                self.hud_shader_gpu = None;
            } else {
                let device = &self.device;
                let atlas_layout = &self.ui_texture_layout;
                let gpu = self.hud_shader_gpu.get_or_insert_with(||
                    HudShaderGpu::new(device, atlas_layout));
                gpu.pipelines.clear();
                gpu.requested.clear();
            }
        }
        if self.ui_state.extension_hud == widgets
            && self.ui_state.native_hidden == hidden
            && self.ui_state.extension_images == images
        {
            return;
        }
        self.ui_state.extension_hud = widgets;
        self.ui_state.native_hidden = hidden;
        self.ui_state.extension_images = images;
        self.sync_ui_extension_images();
        // Uses the existing dynamic HUD buffer/pass. No new egui pass or pipeline.
        self.rebuild_dynamic_ui();
    }

    /// Lazily build only active shader variants on the existing background PSO
    /// worker. No shader-module compilation occurs in normal render frames.
    fn ensure_hud_shader_pipeline(&mut self, name: &str, format: wgpu::TextureFormat) {
        if self.hud_shader_gpu.as_ref().is_none_or(|gpu|
            gpu.pipelines.get(name).is_some_and(|variants| variants.contains_key(&format))) {
            return;
        }
        let Some((_, source)) = self.hud_shader_sources.iter().find(|(key, _)| key == name) else { return };
        let id = crate::renderer::pipeline_hash(&(name, source, format));
        // Each (shader, attachment format) pair needs its own job slot. The
        // worker replaces pending jobs sharing a slot, even if their hashes
        // differ, so the UNORM variant must not evict its sRGB counterpart.
        let slot = crate::renderer::pipeline_hash(&(name, format)) as u32;
        let key = crate::renderer::PipelineJobKey::new("hud-user-wgsl", slot, id);
        if let Some(pipeline) = self.pipeline_jobs.take_ready(key) {
            if let Some(gpu) = self.hud_shader_gpu.as_mut() {
                gpu.pipelines.entry(name.to_string()).or_default().insert(format, pipeline);
            }
            return;
        }
        // Poll only the job state after the initial request. Avoid WGSL or
        // WGPU handle cloning on the render thread while it compiles.
        let Some(gpu) = self.hud_shader_gpu.as_ref() else { return };
        if gpu.requested.get(name).and_then(|variants| variants.get(&format)) == Some(&id) {
            return;
        }
        let device = self.device.clone();
        let layout = gpu.layout.clone();
        let shader_body = source.clone();
        if self.pipeline_jobs.request(key, "Rhai extension HUD WGSL", move || {
            let combined = format!("{}\n{}\n{}",
                include_str!("../hud_extension_shader_header.wgsl"), shader_body,
                include_str!("../hud_extension_shader_footer.wgsl"));
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Rhai extension HUD WGSL"),
                source: wgpu::ShaderSource::Wgsl(combined.into()),
            });
            crate::renderer::create_ui_pipeline(&device, &layout, &shader, format)
        }) {
            if let Some(gpu) = self.hud_shader_gpu.as_mut() {
                gpu.requested.entry(name.to_string()).or_default().insert(format, id);
            }
        }
    }

    /// Poll background shader jobs at UI cadence, not at render frame rate.
    /// Upload only the small retained HUD batch when the integer count changes.
    pub(in crate::renderer) fn refresh_shader_compile_progress(&mut self) {
        let status = self.pipeline_jobs.progress().map(|progress| ui::MapLoadingBar {
            label: "COMPILING SHADERS",
            completed: progress.finished,
            total: progress.total,
            skipped: false,
            no_work: false,
        });
        let stage = if status.is_some() {
            crate::pipeline_jobs::world_pipeline_stage().map(|stage| format!(
                "BSP {}/{} {} ({}s)", stage.pipeline, stage.total,
                stage.description, stage.elapsed_seconds
            ))
        } else {
            None
        };
        // Compile queue just drained: persist the new pipelines now, so a crash
        // or kill later in the session does not lose them.
        if status.is_none() && self.ui_state.shader_compile_progress.is_some() {
            crate::pipeline_cache::save_in_background();
        }
        let signature = |bar: &Option<ui::MapLoadingBar>| {
            bar.as_ref().map(|bar| (bar.completed, bar.total))
        };
        if signature(&self.ui_state.shader_compile_progress) != signature(&status)
            || self.ui_state.shader_compile_stage != stage
        {
            self.ui_state.shader_compile_progress = status;
            self.ui_state.shader_compile_stage = stage;
            self.rebuild_retained_ui();
        }
    }

    pub(in crate::renderer) fn set_player_names(
        &mut self,
        player_names: Option<ui::UiWorldPlayerNames>,
    ) {
        self.player_names = player_names;
    }

    /// Rebuild the UI bind group from the current font/splash/key textures.
    pub(in crate::renderer) fn rebuild_ui_bind_group(&mut self) {
        let splash = if self.ui_binding_is_unknown_map {
            &self._ui_unknown_map
        } else {
            &self._ui_splash
        };
        let images = [
            &self._ui_font, &self._ui_small_font, splash, &self._ui_keys,
            &self._ui_icons, &self._ui_team_icons, &self._ui_ext_images,
            &self._ui_clean_font, &self._ui_avatars,
        ];
        self.ui_bind_group = create_ui_texture_group(
            &self.device, &self.ui_texture_layout, &self._ui_font_sampler, images, false);
        self.ui_encoded_bind_group = create_ui_texture_group(
            &self.device, &self.ui_texture_layout, &self._ui_font_sampler, images, true);
    }

    /// Bind the icon atlas the first time a draw list wants it: the lagometer's
    /// own frame / phone jack, the old speed graph's frame, an image crosshair,
    /// or a weapon / Force selection strip.
    pub(in crate::renderer) fn load_ui_icons_if_needed(&mut self) {
        if self.ui_icons_loaded {
            return;
        }
        let wanted = self
            .ui_state
            .lagometer
            .as_ref()
            .is_some_and(|ui| !ui.pics.is_empty())
            || self
                .ui_state
                .speedometer
                .as_ref()
                .is_some_and(|ui| !ui.graph_pics.is_empty())
            || self.ui_state.crosshair.image != 0
            || self.ui_state.force_select.is_some()
            || self.ui_state.weapon_select.is_some()
            || self.ui_state.inventory_select.is_some()
            || self.ui_state.zoom_scope.is_some()
            || self.ui_state.rocket_lock.is_some()
            || self.ui_state.hud.is_some_and(|hud| {
                hud.force_powers_active != 0 || hud.rage_recovery_until_ms > hud.time_ms
            });
        if !wanted {
            return;
        }
        let started = Instant::now();
        let (data, uvs, flags) = load_ui_icon_atlas(
            &self.base,
            self.detail_texture_game.as_deref(),
            self.device.limits().max_texture_dimension_2d,
        );
        self._ui_icons = upload_texture(&self.device, &self.queue, &data);
        // Keep UV metadata and the uploaded pixels in sync. Incoming app
        // snapshots preserve this render-thread-owned lookup in set_ui().
        self.ui_state.icon_atlas_uvs = uvs;
        self.ui_state.icon_shader_flags = flags;
        self.ui_icon_shader_resolver = None;
        self.ui_icon_shader_materials.clear();
        self.ui_icons_loaded = true;
        self.rebuild_ui_bind_group();
        rverbose!(
            1,
            "UI icon atlas: loaded on first use in {:.1} ms",
            started.elapsed().as_secs_f64() * 1000.0
        );
    }

    /// Repack only when the tinfo/model/item icon set changes. The UI snapshot
    /// carries qpath candidate lists, so all VFS/image work stays on the render
    /// thread and the normal disabled-overlay path remains a 1x1 placeholder.
    pub(in crate::renderer) fn sync_ui_team_icons(&mut self) {
        // One atlas backs every player/class/powerup icon consumer. Keep all
        // active lists in deterministic order so the retained scoreboard and
        // transient team/duel HUD can coexist without rebinding source 6 to a
        // different layout between batches.
        let wanted = ui::shared_player_icon_paths(&self.ui_state);
        if wanted == self.ui_team_icon_key {
            return;
        }
        self.ui_team_icon_key = wanted.clone();
        let data = if wanted.is_empty() {
            empty_ui_font_texture("JKA UI team overlay atlas (not loaded)")
        } else {
            load_ui_team_icon_atlas(&self.base, self.detail_texture_game.as_deref(), &wanted)
        };
        self._ui_team_icons = upload_texture(&self.device, &self.queue, &data);
        self.rebuild_ui_bind_group();
    }


    /// Resolve the ONE enemy-info icon via the ordinary CGame FX shader parser,
    /// not the 64px model-icon atlas. This path understands implicit images,
    /// shader remaps, animMap, ordered stages, blendFuncs, waves and tcMods.
    /// Only an icon *change* does VFS/material work; the presentation clock
    /// selects the already-loaded animation frames thereafter.
    pub(in crate::renderer) fn sync_enemy_icon_shader(&mut self) {
        let wanted = self.ui_state.enemy_info.as_ref()
            .and_then(|enemy| enemy.icon_paths.first()).cloned();
        if self.enemy_icon_key == wanted { return; }
        self.enemy_icon_key = wanted.clone();
        self.enemy_icon_materials.clear();
        let Some(key) = wanted else { return; };
        if self.enemy_icon_resolver.is_none() {
            let Ok(assets) = AssetSearchPath::open_game(
                &self.base, self.detail_texture_game.as_deref(),
            ) else { return; };
            match crate::cgame::entity_presenter::EntityPresenter::new(assets, false) {
                Ok(resolver) => self.enemy_icon_resolver = Some(resolver),
                Err(error) => {
                    eprintln!("Enemy icon shader library: {error}");
                    return;
                }
            }
        }
        let mut candidates = Vec::new();
        for name in key.split('\n').filter(|name| !name.is_empty()) {
            if let Some(spec) = name.strip_prefix("@team-model-icon\t") {
                let mut fields = spec.splitn(5, '\t');
                let (Some(model), Some(skin), Some(team), Some(gametype), Some(jedi_v_merc)) =
                    (fields.next(), fields.next(), fields.next(), fields.next(), fields.next())
                else { continue; };
                let (Ok(team), Ok(gametype)) =
                    (team.parse::<i32>(), gametype.parse::<i32>()) else { continue; };
                let mut final_skin = skin.to_owned();
                if gametype >= crate::cgame::GT_TEAM
                    && gametype != crate::cgame::GT_SIEGE && jedi_v_merc != "1"
                {
                    if let Ok(assets) = AssetSearchPath::open_game(
                        &self.base, self.detail_texture_game.as_deref(),
                    ) {
                        let _ = crate::cgame::validate_skin_for_team(
                            model, &mut final_skin, team,
                            |path| assets.contains_qpath(path),
                        );
                    }
                }
                // CG_LoadClientInfo: full skin first, then composite head.
                candidates.push(format!("models/players/{model}/icon_{final_skin}"));
                if let Some((head, _)) = final_skin.split_once('|') {
                    candidates.push(format!("models/players/{model}/icon_{head}"));
                }
            } else {
                candidates.push(name.to_owned());
            }
        }
        let Some(resolver) = self.enemy_icon_resolver.as_mut() else { return; };
        for candidate in candidates {
            if let Some(materials) = resolver.fx_material_stages_if_present(&candidate) {
                self.enemy_icon_materials = materials;
                rverbose!(2, "Enemy-info shader: {candidate} ({} stages, native resolution)",
                    self.enemy_icon_materials.len());
                break;
            }
        }
    }

    /// Builds only a handful of screen-space shader quads. This is evaluated
    /// on every PRESENT, not only upon receiving a new UI snapshot, so `animMap`
    /// and `rgbGen wave` animate even if the enemy/scoreboard never changes.
    pub(in crate::renderer) fn prepare_enemy_icon_shader(&mut self) {
        self.enemy_icon_batches.clear();
        self.enemy_icon_vertex_count = 0;
        let (w, h) = (self.config.width, self.config.height);
        if self.enemy_icon_materials.is_empty()
            || !ui::gameplay_hud_visible(&self.ui_state, w, h)
            || self.ui_state.enemy_info.is_none()
        { return; }
        let sy = h as f32 / 480.0;
        let sx = sy; // widthRatioCoef * width/640 == height/480
        let rows = usize::from(self.ui_state.video.draw_fps != 0)
            + usize::from(self.ui_state.game_timer.is_some());
        let x0 = w as f32 - 45.0 * sx;
        let y0 = (17.0 + rows as f32 * 22.0) * sy;
        let x1 = x0 + 40.0 * sx;
        let y1 = y0 + 40.0 * sy;
        let seconds = self.frame_time();
        let make_vertex = |x: f32, y: f32, uv: [f32; 2], color: [f32; 4], cut: f32| UiVertex {
            position: [x / w as f32 * 2.0 - 1.0, 1.0 - y / h as f32 * 2.0],
            uv, color,
            // The shared screen-FX fragment shader applies alphaFunc only
            // to specially tagged icon vertices (normal FX always uses 1).
            textured: if cut >= 0.74 { 3.0 } else if cut >= 0.49 { 2.0 }
                else if cut > 0.0 { 4.0 } else if cut < 0.0 { 5.0 } else { 1.0 },
        };
        // Reuse the vertex allocation and the cached stage list: a 2000-FPS
        // icon must not clone all of its animation-frame Arcs every present.
        let mut vertices = std::mem::take(&mut self.enemy_icon_vertices);
        vertices.clear();
        let materials = std::mem::take(&mut self.enemy_icon_materials);
        for stage in materials.iter().take(64) {
            self.ensure_screen_fx_pipeline(stage.blend);
            let texture_key = stage.texture_at_time(seconds)
                .map(|texture| self.ensure_screen_fx_texture(&texture));
            let stage_time = jka_assets::shader::shader_time(
                seconds, stage.shader_time_offset, stage.shader_clamp_time,
            );
            let color = stage.float_vertex_color_at_time([1.0; 4], seconds);
            let uv_for = |uv| stage.tc_mods.iter().fold(uv, |uv, &modifier|
                crate::fx::draw::modified_uv(uv, [0.0; 3], modifier, stage_time));
            let start = vertices.len() as u32;
            vertices.extend_from_slice(&[
                make_vertex(x0, y0, uv_for([0.0, 0.0]), color, stage.alpha_test),
                make_vertex(x0, y1, uv_for([0.0, 1.0]), color, stage.alpha_test),
                make_vertex(x1, y1, uv_for([1.0, 1.0]), color, stage.alpha_test),
                make_vertex(x0, y0, uv_for([0.0, 0.0]), color, stage.alpha_test),
                make_vertex(x1, y1, uv_for([1.0, 1.0]), color, stage.alpha_test),
                make_vertex(x1, y0, uv_for([1.0, 0.0]), color, stage.alpha_test),
            ]);
            self.enemy_icon_batches.push(ScreenFxBatch {
                range: start..start + 6,
                blend: stage.blend,
                texture_key,
            });
        }
        // Same editor element transform as the text, regardless of custom HUD
        // offset/scale. This is intentionally done after shader UV processing.
        let rect_ctx = ui::HudRectContext::from_snapshot(&self.ui_state);
        ui::apply_hud_layout(&mut vertices, ui::HudElementId::EnemyInfo,
            self.ui_state.hud_layout.element(ui::HudElementId::EnemyInfo),
            &rect_ctx, w, h);
        self.enemy_icon_vertex_count = vertices.len() as u32;
        if !vertices.is_empty() {
            self.queue.write_buffer(&self.enemy_icon_vertex_buffer, 0, bytemuck::cast_slice(&vertices));
        }
        self.enemy_icon_vertices = vertices;
        self.enemy_icon_materials = materials;
    }

    /// Scoreboard avatars: on the first board, pack every model's
    /// `icon_default` into one atlas on a worker thread; when it lands, upload
    /// it to binding 9. Returns true on the frame the atlas becomes ready.
    pub(in crate::renderer) fn sync_ui_avatars(&mut self) -> bool {
        if self.ui_avatar_atlas.is_none() && self.ui_avatar_job.is_none() && self.ui_state.scoreboard.is_some() {
            let (sender, receiver) = std::sync::mpsc::channel();
            let base = self.base.clone();
            let game = self.detail_texture_game.clone();
            let spawned = std::thread::Builder::new().name("ui-avatars".into()).spawn(move || {
                let started = Instant::now();
                let (data, atlas) = load_ui_avatar_atlas(&base, game.as_deref());
                eprintln!("UI AVATARS: {} player icons in {:.0} ms", atlas.cells.len(), started.elapsed().as_secs_f64() * 1000.0);
                let _ = sender.send((data, atlas));
            });
            if spawned.is_ok() {
                self.ui_avatar_job = Some(receiver);
            }
        }
        let Some(job) = self.ui_avatar_job.as_ref() else { return false };
        let Ok((data, atlas)) = job.try_recv() else { return false };
        self.ui_avatar_job = None;
        self._ui_avatars = upload_texture(&self.device, &self.queue, &data);
        let atlas = std::sync::Arc::new(atlas);
        self.ui_state.avatar_atlas = Some(atlas.clone());
        self.ui_avatar_atlas = Some(atlas);
        self.rebuild_ui_bind_group();
        true
    }

    /// Resolve Rhai HUD assets off-thread. First-use shader-library parsing and
    /// texture decoding never block a frame. The existing atlas and stable
    /// slots remain visible until each background batch is ready for upload.
    pub(in crate::renderer) fn sync_ui_extension_images(&mut self) {
        let wanted = self.ui_state.extension_images.clone();
        if wanted.is_empty() {
            self.ui_ext_image_loader = None;
            if !self.ui_ext_image_key.is_empty() {
                self._ui_ext_images = upload_texture(
                    &self.device, &self.queue,
                    &empty_ui_font_texture("JKA UI extension image atlas (not loaded)"),
                );
                self.ui_ext_image_key.clear();
                self.ui_state.extension_shader_flags.clear();
                self.ui_state.extension_image_dimensions.clear();
                self.ui_icon_shader_materials.retain(|&key, _| key < ui::ICON_NAMES.len());
                self.rebuild_ui_bind_group();
            }
            return;
        }

        // A non-prefix update is a full HUD reload, not a request to reassign
        // existing cell indices. Results from the old request are discarded.
        let atlas_rows = ui_extension_atlas_rows(wanted.len());
        let atlas_height = atlas_rows * ui::EXT_IMAGE_CELL;
        // Native JK2 can need more artwork than the 32 script-controlled slots.
        // Grow the GPU atlas *only when needed*, never on the competitive HUD path.
        if !wanted.starts_with(&self.ui_ext_image_key)
            || self._ui_ext_images._texture.size().height != atlas_height
        {
            self._ui_ext_images = upload_texture(
                &self.device, &self.queue, &blank_ui_extension_atlas_for_count(wanted.len()),
            );
            self.ui_ext_image_key.clear();
            self.ui_ext_image_loader = None;
            self.ui_state.extension_shader_flags.clear();
            self.ui_state.extension_image_dimensions.clear();
            self.ui_icon_shader_materials.retain(|&key, _| key < ui::ICON_NAMES.len());
            self.rebuild_ui_bind_group();
        }

        let completed = self.ui_ext_image_loader.as_mut().and_then(|loader| {
            match loader.results.try_recv() {
                Ok(batch) => { loader.pending = false; Some(batch) }
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    loader.pending = false;
                    None
                }
            }
        });
        if let Some(batch) = completed {
            if batch.first_new == self.ui_ext_image_key.len()
                && wanted.starts_with(&batch.wanted)
            {
                let started = Instant::now();
                let cell = ui::EXT_IMAGE_CELL;
                for (offset, pixels) in batch.pixels.iter().enumerate() {
                    let Some(pixels) = pixels else { continue };
                    let index = (batch.first_new + offset) as u32;
                    self.queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &self._ui_ext_images._texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d {
                                x: (index % ui::EXT_IMAGE_COLUMNS) * cell,
                                y: (index / ui::EXT_IMAGE_COLUMNS) * cell,
                                z: 0,
                            },
                            aspect: wgpu::TextureAspect::All,
                        },
                        pixels,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(cell * 4),
                            rows_per_image: Some(cell),
                        },
                        wgpu::Extent3d { width: cell, height: cell, depth_or_array_layers: 1 },
                    );
                }
                self.ui_state.extension_shader_flags.extend(batch.shader_flags);
                self.ui_state.extension_image_dimensions.extend(batch.source_dimensions);
                for (index, stages) in batch.shader_stages {
                    self.ui_icon_shader_materials.insert(index, stages);
                }
                self.ui_ext_image_key = batch.wanted;
                // These vertices were built when the new shader flags were not
                // yet available; retag them now for the authored-stage path.
                self.rebuild_dynamic_ui();
                rverbose!(1, "UI extension images: {} async-loaded, upload {:.2} ms ({} total)",
                    batch.pixels.len(), started.elapsed().as_secs_f64() * 1000.0,
                    self.ui_ext_image_key.len());
            }
        }

        if wanted == self.ui_ext_image_key { return; }
        if self.ui_ext_image_loader.is_none() {
            // Atlas allocation already matches wanted.len() above.
            self.ui_ext_image_loader = Some(UiExtImageLoader::start(
                self.base.clone(), self.detail_texture_game.clone(),
            ));
        }
        let Some(loader) = self.ui_ext_image_loader.as_mut() else { return; };
        if loader.pending { return; }
        let first_new = self.ui_ext_image_key.len();
        if !loader.enqueue(wanted, first_new) {
            // Pool is temporarily saturated or unavailable. Retry on the next
            // UI sync rather than blocking the render thread or losing icons.
            rverbose!(2, "Extension HUD asset pool unavailable; retrying");
        }
    }

    /// Read the movement key images and bind them. Runs once, the first time the
    /// movement keys are switched on, instead of at every startup.
    pub(in crate::renderer) fn load_ui_key_atlas_now(&mut self) {
        let started = Instant::now();
        let data = load_ui_key_atlas(&self.base, self.detail_texture_game.as_deref());
        self._ui_keys = upload_texture(&self.device, &self.queue, &data);
        self.ui_keys_loaded = true;
        self.rebuild_ui_bind_group();
        rverbose!(
            1,
            "UI movement keys: atlas loaded on first use in {:.1} ms",
            started.elapsed().as_secs_f64() * 1000.0
        );
    }

    pub(in crate::renderer) fn sync_ui_loading_background(&mut self) {
        let request = self.ui_state.loading.as_ref().and_then(|loading| {
            let stem = loading_levelshot_stem(&loading.map_name)?;
            Some((stem, loading.active_game_dir.as_ref().map(PathBuf::from)))
        });
        let desired_key = request.as_ref().map(|(stem, game)| {
            format!(
                "{}\n{}",
                game.as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
                stem
            )
        });
        if self.ui_loading_image_key == desired_key {
            return;
        }

        self.ui_loading_image_key = desired_key.clone();
        self.ui_pending_levelshot = None;

        let Some((map_name, game)) = request else {
            // Binding 3 is not drawn outside the retained loading screen. Avoid
            // hitting the VFS just to restore menu/splash when loading ends.
            return;
        };
        let key = desired_key.expect("loading request must have a key");

        // First present the already-resident OpenJK MP unknown-map art. The real
        // levelshot is deliberately resolved only after this frame reaches present().
        self.ui_splash_size = self.ui_unknown_map_size;
        self.ui_binding_is_unknown_map = true;
        self.rebuild_ui_bind_group();
        self.ui_pending_levelshot = Some(PendingUiLevelshot {
            key,
            map_name,
            game,
        });
    }

    /// Resolve the real map levelshot only after at least one fallback frame has
    /// already reached `present()`. If disk/decode is slow, the compositor keeps
    /// showing `unknownmap_mp` instead of exposing the renderer clear color.
    pub(in crate::renderer) fn resolve_pending_ui_levelshot(&mut self) {
        // The startup splash deliberately owns the very first presented frame.
        // Do not consume a queued map levelshot until the actual loading UI
        // (and therefore unknownmap_mp) has itself reached present() once.
        if self.ui_state.startup_splash || self.ui_state.loading.is_none() {
            return;
        }
        let Some(pending) = self.ui_pending_levelshot.take() else {
            return;
        };
        if self.ui_loading_image_key.as_deref() != Some(pending.key.as_str()) {
            return;
        }

        // TaystJK supports dedicated 16:9 levelshots. Prefer them when the
        // viewport is wide, then fall back to the stock 4:3 qpath. Neither
        // image is ever cover-cropped by the native loading renderer.
        let path = format!("levelshots/{}", pending.map_name);
        let wide = format!("levelshots_16_9/{}", pending.map_name);
        let prefer_wide = self.config.width as f32 / self.config.height.max(1) as f32 >= 1.5;
        let data = (if prefer_wide {
            try_load_texture_asset(&self.base,pending.game.as_deref(),&wide,
                true,false,true,"UI widescreen levelshot")
        } else {None}).or_else(||try_load_texture_asset(
            &self.base,pending.game.as_deref(),&path,
            true,false,true,"UI levelshot"));
        let Some(data) = data else {
            eprintln!("UI levelshot: no image resolved for {path}; keeping unknown-map art");
            return;
        };

        let size = [data.width, data.height];
        let image = upload_texture(&self.device, &self.queue, &data);
        self.ui_splash_size = Some(size);
        self._ui_splash = image;
        self.ui_binding_is_unknown_map = false;
        self.rebuild_ui_bind_group();
        // Rebuild the retained UI after changing the bound artwork.
        self.rebuild_retained_ui();
    }

    /// The legacy 2D material uses the encoded component values of an SDR
    /// target. Swapchain aliases need surface support; owned offscreen textures
    /// explicitly advertise the matching UNORM view without allocating pixels.
    /// Native HDR's existing Rgba8Unorm UI canvas is also an encoded target;
    /// its established compositor decodes the complete premultiplied UI layer.
    pub(in crate::renderer) fn native_2d_encoded_surface_eligible(&self) -> bool {
        self.hdr_output.is_some() || (self.config.format.is_srgb()
            && (self.surface_unorm_alias_supported || self.offscreen_output.is_some()))
    }

    /// Prepare both ordinary and authored shader stages together. Until all
    /// required lazy PSOs are ready, draw the *whole* overlay with the normal
    /// sRGB pipeline so no mixed-space stages or pipeline validation errors occur.
    pub(in crate::renderer) fn ensure_encoded_2d_pipelines(&mut self) -> bool {
        let format = if self.hdr_output.is_some() {
            wgpu::TextureFormat::Rgba8Unorm
        } else {
            self.config.format.remove_srgb_suffix()
        };
        if self.ui_encoded_format != Some(format) {
            // HDR switching changes the output format (BGRA to RGBA).
            // Keep no stale PSOs around; each async job is keyed by its format.
            self.ui_encoded_pipeline = None;
            self.screen_fx_encoded_pipelines.clear();
            if let Some(gamma) = self.ui_gamma_native.as_mut() {
                gamma.encoded_pipeline = None;
                gamma.encoded_fx_pipelines.clear();
            }
            self.ui_encoded_format = Some(format);
        }
        // HardwareSceneOnly must not disable the encoded 2D target. Prepare a
        // gamma-aware variant of BOTH the implicit HUD material and authored
        // screen-FX stages, all targeting the same UNORM view without a copy.
        let gamma_active = self.ui_gamma_active();
        if gamma_active {
            self.ensure_native_ui_gamma();
        }
        if gamma_active {
            let gamma = self.ui_gamma_native.as_mut().expect("native UI gamma resources");
            if gamma.encoded_pipeline.is_none() {
                let key = PipelineJobKey::new("ui-encoded-hw-gamma", 0, pipeline_hash(&format));
                if let Some(pipeline) = self.pipeline_jobs.take_ready(key) {
                    gamma.encoded_pipeline = Some(pipeline);
                } else {
                    let device = self.device.clone();
                    let layout = gamma.pipeline_layout.clone();
                    let shader = gamma.shader.clone();
                    self.pipeline_jobs.request(key, "native encoded UI + hardware gamma", move || {
                        crate::renderer::create_ui_pipeline(&device, &layout, &shader, format)
                    });
                }
            }
            let mut needed = [None; 32];
            for batch in self.screen_fx_batches.iter().chain(self.enemy_icon_batches.iter()) {
                if let Some(slot) = needed.get_mut(batch.blend as usize) { *slot = Some(batch.blend); }
            }
            for batch in &self.ui_icon_shader_batches {
                if let Some(slot) = needed.get_mut(batch.stage.blend as usize) { *slot = Some(batch.stage.blend); }
            }
            let mut all_ready = gamma.encoded_pipeline.is_some();
            for blend in needed.into_iter().flatten() {
                if gamma.encoded_fx_pipelines.contains_key(&blend) { continue; }
                let key = PipelineJobKey::new("screen-fx-encoded-hw-gamma", blend as u32,
                    pipeline_hash(&(format, blend)));
                if let Some(pipeline) = self.pipeline_jobs.take_ready(key) {
                    gamma.encoded_fx_pipelines.insert(blend, pipeline);
                } else {
                    let device = self.device.clone();
                    let layout = gamma.fx_pipeline_layout.clone();
                    let shader = gamma.fx_shader.clone();
                    self.pipeline_jobs.request(key, "authored 2D stages + hardware gamma", move || {
                        create_screen_fx_pipeline(&device, &layout, &shader, format, blend)
                    });
                    all_ready = false;
                }
            }
            return all_ready;
        }
        if self.ui_encoded_pipeline.is_none() {
            let key = PipelineJobKey::new("ui-encoded", 0, pipeline_hash(&format));
            if let Some(pipeline) = self.pipeline_jobs.take_ready(key) {
                self.ui_encoded_pipeline = Some(pipeline);
            } else {
                let device = self.device.clone();
                let layout = self.ui_pipeline_layout.clone();
                let shader = self.ui_shader.clone();
                self.pipeline_jobs.request(key, "native UI legacy 2D blend", move || {
                    crate::renderer::create_ui_pipeline(&device, &layout, &shader, format)
                });
            }
        }
        // Keep this allocation-free at competitive frame rates. Discover
        // every blend variant requested by the authored materials, including
        // variants added by future shader parsers (e.g. SCREEN blending).
        let mut needed = [None; 32];
        for batch in self.screen_fx_batches.iter().chain(self.enemy_icon_batches.iter()) {
            if let Some(slot) = needed.get_mut(batch.blend as usize) { *slot = Some(batch.blend); }
        }
        for batch in &self.ui_icon_shader_batches {
            if let Some(slot) = needed.get_mut(batch.stage.blend as usize) { *slot = Some(batch.stage.blend); }
        }
        let mut all_ready = self.ui_encoded_pipeline.is_some();
        for blend in needed.into_iter().flatten() {
            if self.screen_fx_encoded_pipelines.contains_key(&blend) { continue; }
            let key = PipelineJobKey::new("screen-fx-encoded", blend as u32,
                pipeline_hash(&(format, blend)));
            if let Some(pipeline) = self.pipeline_jobs.take_ready(key) {
                self.screen_fx_encoded_pipelines.insert(blend, pipeline);
            } else {
                let device = self.device.clone();
                let layout = self.screen_fx_pipeline_layout.clone();
                let shader = self.screen_fx_shader.clone();
                self.pipeline_jobs.request(key, "authored HUD legacy 2D blend", move || {
                    create_screen_fx_pipeline(&device, &layout, &shader, format, blend)
                });
                all_ready = false;
            }
        }
        all_ready
    }

    pub(in crate::renderer) fn ensure_screen_fx_pipeline(&mut self, blend: FxBlend) {
        if self.screen_fx_pipelines.contains_key(&blend) {
            return;
        }
        let key = PipelineJobKey::new(
            "screen-fx",
            blend as u32,
            pipeline_hash(&(self.config.format, blend)),
        );
        if let Some(pipeline) = self.pipeline_jobs.take_ready(key) {
            self.screen_fx_pipelines.insert(blend, pipeline);
            return;
        }
        let device = self.device.clone();
        let layout = self.screen_fx_pipeline_layout.clone();
        let shader = self.screen_fx_shader.clone();
        let format = self.config.format;
        self.pipeline_jobs
            .request(key, "screen FX blend variant", move || {
                create_screen_fx_pipeline(&device, &layout, &shader, format, blend)
            });
    }

    pub(in crate::renderer) fn ensure_screen_fx_texture(
        &mut self,
        texture: &Arc<TextureData>,
    ) -> String {
        let key = format!(
            "{}|{}|{}x{}|{}|clamp={}",
            texture.label,
            texture
                .source
                .as_ref()
                .map_or_else(String::new, |path| path.display().to_string()),
            texture.width,
            texture.height,
            texture.mip_level_count,
            texture.clamp,
        );
        if !self.screen_fx_textures.contains_key(&key) {
            let image = upload_texture(&self.device, &self.queue, texture);
            self.baked_brightness.register(&image._texture);
            let sampler = if texture.clamp { &self.screen_fx_sampler } else { &self.screen_fx_repeat_sampler };
            let make_group = |view: &wgpu::TextureView| {
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("JKA CGame screen FX bind group"),
                    layout: &self.screen_fx_texture_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
                    ],
                })
            };
            let bind_group = make_group(&image.view);
            let encoded_view = encoded_image_view(&image);
            let encoded_bind_group = make_group(&encoded_view);
            self.screen_fx_textures.insert(key.clone(), ScreenFxTexture {
                _image: image, bind_group, encoded_bind_group,
            });
        }
        key
    }

    pub(in crate::renderer) fn set_screen_fx(&mut self, draws: &[crate::fx::draw::ScreenFxDraw]) {
        self.screen_fx_batches.clear();
        self.screen_fx_vertex_count = 0;
        if draws.is_empty() {
            return;
        }

        let max_vertices = SCREEN_FX_BUFFER_BYTES as usize / std::mem::size_of::<UiVertex>();
        let mut vertices = Vec::with_capacity((draws.len() * 6).min(max_vertices));
        for draw in draws {
            if vertices.len().saturating_add(6) > max_vertices {
                break;
            }
            self.ensure_screen_fx_pipeline(draw.material.blend);
            let texture_key = draw
                .material
                .texture
                .as_ref()
                .map(|texture| self.ensure_screen_fx_texture(texture));

            let [mut x, y, mut w, h] = draw.rect;
            if draw.anchor != crate::fx::draw::ScreenFxAnchor::Stretch {
                // widthRatioCoef = (640 * height) / (480 * width)
                let coef =
                    (640.0 * self.config.height as f32) / (480.0 * self.config.width.max(1) as f32);
                match draw.anchor {
                    crate::fx::draw::ScreenFxAnchor::Center => x = 320.0 + (x - 320.0) * coef,
                    crate::fx::draw::ScreenFxAnchor::Right => x = 640.0 - (640.0 - x) * coef,
                    crate::fx::draw::ScreenFxAnchor::Stretch => {}
                }
                w *= coef;
            }
            let x0 = x / 640.0 * 2.0 - 1.0;
            let x1 = (x + w) / 640.0 * 2.0 - 1.0;
            let y0 = 1.0 - y / 480.0 * 2.0;
            let y1 = 1.0 - (y + h) / 480.0 * 2.0;
            let color = draw.material.float_vertex_color(draw.color);
            let [u0, v0, u1, v1] = draw.uv_rect;
            let vertex = |position, uv| UiVertex {
                position,
                uv,
                color,
                textured: 1.0,
            };
            let start = vertices.len() as u32;
            vertices.extend_from_slice(&[
                vertex([x0, y0], [u0, v0]),
                vertex([x0, y1], [u0, v1]),
                vertex([x1, y1], [u1, v1]),
                vertex([x0, y0], [u0, v0]),
                vertex([x1, y1], [u1, v1]),
                vertex([x1, y0], [u1, v0]),
            ]);
            self.screen_fx_batches.push(ScreenFxBatch {
                range: start..start + 6,
                blend: draw.material.blend,
                texture_key,
            });
        }
        self.screen_fx_vertex_count = vertices.len() as u32;
        if !vertices.is_empty() {
            self.queue.write_buffer(
                &self.screen_fx_vertex_buffer,
                0,
                bytemuck::cast_slice(&vertices),
            );
        }
    }

    pub(in crate::renderer) fn set_dynamic_hud(
        &mut self,
        hud: Option<ui::HudState>,
        movement_hud: ui::MovementHudState,
    ) {
        let sight_before = self.ui_state.hud.as_ref().is_some_and(|hud|
            hud.force_powers_active & (1 << 14) != 0);
        let sight_after = hud.as_ref().is_some_and(|hud|
            hud.force_powers_active & (1 << 14) != 0);
        let hud_changed = self.ui_state.hud != hud;
        let movement_changed = self.ui_state.movement_hud != movement_hud;
        if !hud_changed && !movement_changed {
            return;
        }
        self.ui_state.hud = hud;
        self.ui_state.movement_hud = movement_hud;
        if sight_before != sight_after {
            // PVS selection may be unchanged, but GPU visibility must update
            // immediately when Force Seeing starts or stops.
            self.update_gpu_cull_settings();
            // Cached static sun caster layers were built with the old scene
            // visibility. Rebuild once after a Force Seeing transition.
            self.shadow_resources.static_cascades = [None; crate::renderer::SHADOW_CASCADES];
        }
        if hud_changed {
            // HUD text/geometry can change size, so rebuild the stable prefix and
            // then append the current movement tail.
            self.rebuild_dynamic_ui();
        } else {
            // The common path: only velocity/input/view state changed. Preserve
            // the stable prefix and rewrite the tiny movement tail in place.
            self.rebuild_movement_ui();
        }
    }

    pub(in crate::renderer) fn set_transient_ui(
        &mut self,
        chat_lines: Vec<ui::UiChatLine>,
        console_notify_lines: Vec<ui::UiConsoleNotifyLine>,
        center_print: Option<ui::UiCenterPrint>,
        demo_timeline: Option<ui::DemoTimelineUi>,
        prediction_debug: Option<ui::PredictionDebugUi>,
        crosshair_target: ui::UiCrosshairTarget,
        force_select: Option<ui::UiForceSelect>,
        weapon_select: Option<ui::UiWeaponSelect>,
        inventory_select: Option<ui::UiInventorySelect>,
        follow_name: Option<String>,
        game_timer: Option<String>,
        mini_scores: Option<ui::UiMiniScores>,
        hud_score: Option<ui::UiHudScore>,
        enemy_info: Option<ui::UiEnemyInfo>,
        race_timer: Option<crate::japro_cg::RaceTimerUi>,
        vote_line: Option<String>,
        siege_hud: Option<ui::UiSiegeHud>,
        scoreboard: Option<ui::UiScoreboard>,
        scoreboard_focus_client: Option<i32>,
        killfeed: Option<ui::UiKillfeed>,
        chat_anim: ui::UiChatAnim,
        speedometer: Option<crate::speedometer::Ui>,
        lagometer: Option<crate::lagometer::Ui>,
        team_overlay: Option<ui::TeamOverlayUi>,
    ) {
        self.ui_state.chat_lines = chat_lines;
        self.ui_state.console_notify_lines = console_notify_lines;
        self.ui_state.center_print = center_print;
        self.ui_state.demo_timeline = demo_timeline;
        self.ui_state.prediction_debug = prediction_debug;
        self.ui_state.crosshair_target = crosshair_target;
        self.ui_state.force_select = force_select;
        self.ui_state.weapon_select = weapon_select;
        self.ui_state.inventory_select = inventory_select;
        self.ui_state.follow_name = follow_name;
        self.ui_state.game_timer = game_timer;
        self.ui_state.mini_scores = mini_scores;
        self.ui_state.hud_score = hud_score;
        self.ui_state.enemy_info = enemy_info;
        self.ui_state.race_timer = race_timer;
        self.ui_state.vote_line = vote_line;
        self.ui_state.siege_hud = siege_hud;
        let scoreboard_changed = self.ui_state.scoreboard != scoreboard
            || self.ui_state.scoreboard_focus_client != scoreboard_focus_client;
        self.ui_state.scoreboard = scoreboard;
        self.ui_state.scoreboard_focus_client = scoreboard_focus_client;
        self.ui_state.killfeed = killfeed;
        self.ui_state.chat_anim = chat_anim;
        let avatars_ready = self.sync_ui_avatars();
        self.ui_state.speedometer = speedometer;
        self.ui_state.lagometer = lagometer;
        self.ui_state.team_overlay = team_overlay;
        self.load_ui_icons_if_needed();
        self.sync_ui_team_icons();
        self.sync_enemy_icon_shader();
        self.sync_ui_extension_images();
        if scoreboard_changed || avatars_ready {
            // Scoreboard visibility and focus are transient state. Rebuild its
            // retained batch only when either input changes.
            self.rebuild_retained_ui();
        }
        self.rebuild_transient_ui();
    }

    pub(in crate::renderer) fn set_ui_telemetry(
        &mut self,
        perf: ui::PerfStats,
        threads: std::sync::Arc<[ui::ThreadPerfStats]>,
    ) {
        self.ui_state.perf = perf;
        self.ui_state.threads = threads;
        // Telemetry is part of the small volatile overlay batch. Rebuilding it
        // never touches retained console/menu/scoreboard geometry.
        if self.ui_state.video.draw_fps != 0 {
            self.rebuild_transient_ui();
        }
    }

    pub(in crate::renderer) fn set_egui(&mut self, frame: Option<EguiRenderData>) {
        let Some(mut frame) = frame else {
            self.egui_active = false;
            self.egui_present_token = None;
            self.egui_paint_jobs.clear();
            for texture_id in self.egui_pending_free.drain(..) {
                self.egui_renderer.free_texture(&texture_id);
            }
            return;
        };

        for (texture_id, image_deltas) in &frame.textures_delta.set {
            for image_delta in image_deltas {
                self.egui_renderer
                    .update_texture(&self.device, &self.queue, *texture_id, image_delta);
            }
        }
        self.egui_pending_free.extend(frame.textures_delta.free.iter().copied());
        frame.textures_delta.clear();
        self.egui_paint_jobs = std::mem::take(&mut frame.paint_jobs);
        self.egui_pixels_per_point = frame.pixels_per_point.max(0.25);
        self.egui_active = true;
        self.egui_present_token = frame.present_token;
    }

    pub(in crate::renderer) fn submit_egui(&mut self, frame_view: &wgpu::TextureView) {
        self.submit_egui_impl(frame_view, false, false);
    }

    pub(in crate::renderer) fn submit_egui_deferred_hdr(&mut self, frame_view: &wgpu::TextureView) {
        self.submit_egui_impl(frame_view, true, false);
    }

    /// Loading dialogs are painted over the native loading screen. In encoded
    /// HDR mode that screen and egui share the same UI texture: don't clear
    /// the native bars before painting the dialog over them.
    pub(in crate::renderer) fn submit_egui_deferred_hdr_over_native(&mut self, frame_view: &wgpu::TextureView) {
        self.submit_egui_impl(frame_view, true, true);
    }

    fn submit_egui_impl(&mut self, frame_view: &wgpu::TextureView, defer_hdr_composite: bool,
        preserve_native_hdr_ui: bool) {
        if !self.egui_active || (self.egui_paint_jobs.is_empty() && !self.neon_chrome_wanted()) {
            return;
        }

        // This allocation and extra overlay pass are never used during normal
        // gameplay. Unlike CPU texture baking, this also handles text and
        // egui user textures, including material preview thumbnails.
        let compensate_ui = self.ui_gamma_active() && self.hdr_output.is_none();
        if compensate_ui { self.ensure_egui_ui_gamma(); }
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width.max(1), self.config.height.max(1)],
            pixels_per_point: self.egui_pixels_per_point,
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("JKA egui overlay encoder"),
            });
        let callback_command_buffers = self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &self.egui_paint_jobs,
            &screen,
        );
        // The footer and submenu typography are actual WGPU neon tubes.
        // Composite into egui's UI target *before* widgets. For the gamma/HDR
        // intermediates both passes must share the same transparent layer.
        let chrome_target = if compensate_ui {
            &self.ui_gamma_egui.as_ref().expect("egui gamma target").view
        } else {
            self.hdr_output.as_ref().map_or(frame_view, |output| &output.ui_view)
        }.clone();
        let chrome_format = if self.hdr_output.is_some() { wgpu::TextureFormat::Rgba8Unorm }
            else { self.config.format };
        let chrome_drawn = self.encode_neon_chrome(&mut encoder, &chrome_target,
            chrome_format, compensate_ui || self.hdr_output.is_some());
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("JKA egui overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: if compensate_ui {
                        &self.ui_gamma_egui.as_ref().expect("egui gamma target").view
                    } else {
                        self.hdr_output.as_ref().map_or(frame_view, |output| &output.ui_view)
                    },
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if (self.hdr_output.is_some() || compensate_ui)
                            && !chrome_drawn && !preserve_native_hdr_ui {
                            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            self.egui_renderer
                .render(&mut pass, &self.egui_paint_jobs, &screen);
        }
        if compensate_ui {
            let resources = self.ui_gamma_egui.as_ref().expect("egui gamma compositor");
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("HW scene-only egui overlay composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: frame_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&resources.pipeline);
            pass.set_bind_group(0, &resources.group, &[]);
            pass.draw(0..3, 0..1);
        }
        if !defer_hdr_composite {
            if let Some(output) = &self.hdr_output {
                output.composite_ui(&mut encoder, frame_view);
            }
        }
        let egui_command_buffer = encoder.finish();
        self.queue.submit(
            callback_command_buffers
                .into_iter()
                .chain(std::iter::once(egui_command_buffer)),
        );

        for texture_id in self.egui_pending_free.drain(..) {
            self.egui_renderer.free_texture(&texture_id);
        }
    }

    pub(in crate::renderer) fn menu_backdrop_requested(&self) -> bool {
        // In Studio the recorded world is composited separately; blurring its
        // fullscreen backing image would defeat viewport isolation.
        if self.studio_world_viewport.is_some() { return false; }
        // The frontend Main scene is artwork, not a page that needs a blurred
        // and desaturated background. Avoid both the grading and its extra pass.
        // It does use the same lazily allocated scene capture as the source of
        // the renderer-drawn main page (neon_menu.rs).
        self.neon_menu_wanted()
            || (!self.ui_state.frontend_main_scene
                && !self.ui_state.video.skip_ui
                && self.ui_state.loading.is_none()
                && (self.ui_state.mode == ui::OverlayMode::Game
                    || (self.ui_state.mode == ui::OverlayMode::Video
                        && self.ui_state.setup_selected != 1)))
    }

    /// The scene may render into the backdrop source this frame. The main page
    /// waits for its own pipelines too, rendering straight to the frame until then.
    pub(in crate::renderer) fn menu_backdrop_ready(&self) -> bool {
        self.menu_backdrop.is_some() && (!self.neon_menu_wanted() || self.neon_menu_ready())
    }

    pub(in crate::renderer) fn ensure_menu_backdrop_resources(&mut self) {
        let width = self.config.width.max(1);
        let height = self.config.height.max(1);
        let format = self.config.format;
        if self
            .menu_backdrop
            .as_ref()
            .is_some_and(|resources| resources.width == width && resources.height == height)
        {
            return;
        }
        // Never use an old-size backdrop while its replacement is compiling.
        self.menu_backdrop = None;
        let key = PipelineJobKey::new("menu-backdrop", 0, pipeline_hash(&(width, height, format)));
        if let Some(resources) = self.pipeline_jobs.take_ready::<MenuBackdropResources>(key) {
            self.menu_backdrop = Some(resources);
            return;
        }
        let device = self.device.clone();
        let sampler = self.post_sampler.clone();
        self.pipeline_jobs.request(key, "menu backdrop", move || {
            create_menu_backdrop_resources(&device, &sampler, width, height, format)
        });
    }

    /// `plain_scene`: the frame of a cached-scene capture, which must not
    /// contain the main page's overlays.
    pub(in crate::renderer) fn submit_menu_backdrop(&self, frame_view: &wgpu::TextureView, plain_scene: bool) {
        if self.neon_menu_wanted() && self.submit_neon_menu(frame_view, plain_scene) {
            return;
        }
        let Some(backdrop) = &self.menu_backdrop else {
            return;
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("JKA menu backdrop encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("JKA menu Gaussian backdrop"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: frame_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&backdrop.pipeline);
            pass.set_bind_group(0, &backdrop.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
    }

    /// Resolve only visible authored HUD shaders, then evaluate their stages at
    /// presentation time. The source UI vertex stream stays intact, and the
    /// painter-order splitter below removes authored quads from the atlas pass.
    pub(in crate::renderer) fn prepare_ui_icon_shaders(&mut self) {
        self.ui_icon_shader_batches.clear();
        self.ui_icon_shader_vertices.clear();
        if let Some(gpu) = self.hud_shader_gpu.as_mut() { gpu.pending.clear(); }
        if self.ui_state.video.skip_ui ||
            !(self.ui_icons_loaded && self.ui_state.icon_shader_flags.iter().any(|&flag| flag)
              || self.ui_state.extension_shader_flags.iter().any(|&flag| flag)
              || !self.hud_shader_sources.is_empty())
        { return; }
        // Collect only the tagged triangles. In practice this is the small
        // handful of scope/Force icons on screen, not the whole HUD.
        let mut segments: Vec<(u8, u32, usize, Vec<UiVertex>)> = Vec::new();
        for (layer, source) in [
            (0, self.ui_dynamic_vertices.as_slice()),
            (1, self.ui_transient_vertices.as_slice()),
            (2, self.ui_retained_vertices.as_slice()),
        ] {
            let mut i = 0;
            while i + 3 <= source.len() {
                let mark = source[i].textured;
                if let Some(index) = ui_shader_material_index(mark) {
                    let start = i;
                    i += 3;
                    while i + 3 <= source.len()
                        && ui_shader_material_index(source[i].textured) == Some(index)
                    { i += 3; }
                    segments.push((layer, start as u32, index, source[start..i].to_vec()));
                } else { i += 3; }
            }
        }
        if segments.is_empty() { return; }
        // Native authored icons still use the existing on-demand resolver;
        // extension stages arrive pre-resolved from the HUD image worker.
        // Reuse resolved frame textures/PSOs. The resolver is never used in
        // the no-shader path, nor on subsequent frames for the same icon.
        let seconds = self.frame_time();
        let gamma_active = self.ui_gamma_active();
        let srgb_output = self.config.format.is_srgb();
        for (layer, original_start, index, quad) in segments {
            let original_end = original_start + quad.len() as u32;
            if index >= USER_SHADER_INDEX_BASE {
                let command_index = index - USER_SHADER_INDEX_BASE;
                let widget_index = command_index / crate::extensions::MAX_HUD_COMMANDS;
                let command_index = command_index % crate::extensions::MAX_HUD_COMMANDS;
                let command = self.ui_state.extension_hud.get(widget_index)
                    .and_then(|w| w.widget.commands.get(command_index));
                if let Some(crate::extensions::HudCommand::ShaderImage {
                    name, shader, params, ..
                }) = command {
                    let found = self.hud_shader_sources.iter().any(|(key, _)| key == shader);
                    let source_index = self.ui_state.extension_images.iter().position(|v| v == name);
                    if found {
                        if let (Some(source_index), Some(gpu)) = (source_index, self.hud_shader_gpu.as_mut()) {
                            let (uv0, uv1) = ui::extension_image_cell_uv_for_count(source_index, self.ui_state.extension_images.len());
                            let uniform = HudShaderUniform {
                                atlas_rect: [uv0[0], uv0[1], uv1[0]-uv0[0], uv1[1]-uv0[1]],
                                values: *params,
                                clock: [seconds, self.gamma,
                                    if gamma_active { 1.0 } else { 0.0 },
                                    if srgb_output { 1.0 } else { 0.0 }],
                            };
                            if let Some(offset) = gpu.append(uniform) {
                                if self.ui_icon_shader_vertices.len() + quad.len() <= 16_384 {
                                    let start = self.ui_icon_shader_vertices.len() as u32;
                                    self.ui_icon_shader_vertices.extend(quad.iter().map(|v| {
                                        let mut v = *v;
                                        v.textured = ui::EXT_IMAGE_TEXTURE_SOURCE;
                                        v
                                    }));
                                    self.ui_icon_shader_batches.push(UiIconShaderBatch {
                                        layer, original_start, original_end,
                                        stage: ScreenFxBatch {
                                            range: start..self.ui_icon_shader_vertices.len() as u32,
                                            blend: FxBlend::Alpha, texture_key: None,
                                        },
                                        user: Some((shader.clone(), offset)),
                                    });
                                    continue;
                                }
                            }
                        }
                    }
                }
                // Missing shader, image, or budget: suppress the custom tag.
                // The shader is optional and cannot corrupt the stock pipeline.
                continue;
            }
            // Every shader-tagged span must be excluded from the native UI
            // pipeline even if its authored material is missing/unrenderable.
            self.ui_icon_shader_batches.push(UiIconShaderBatch {
                layer, original_start, original_end,
                stage: ScreenFxBatch {
                    range: 0..0, blend: FxBlend::Alpha, texture_key: None,
                },
                user: None,
            });
            if !self.ui_icon_shader_materials.contains_key(&index) {
                if self.ui_icon_shader_resolver.is_none() {
                    let Ok(assets) = AssetSearchPath::open_game(
                        &self.base, self.detail_texture_game.as_deref(),
                    ) else { continue; };
                    match crate::cgame::entity_presenter::EntityPresenter::new(assets, false) {
                        Ok(resolver) => self.ui_icon_shader_resolver = Some(resolver),
                        Err(error) => { eprintln!("HUD shader resolver: {error}"); continue; }
                    }
                }
                let is_extension = index >= ui::ICON_NAMES.len();
                let name = if is_extension {
                    self.ui_state.extension_images.get(index - ui::ICON_NAMES.len()).cloned()
                } else {
                    Some(ui::icon_name(index).into_owned()).filter(|name| !name.is_empty())
                };
                let Some(name) = name else { continue; };
                let name = name.as_str();
                let requested = if !is_extension &&
                    (ui::ICON_WEAPON_NO_AMMO_BASE..ui::ICON_WEAPON_STAFF).contains(&index)
                { format!("{name}_na") } else { name.to_owned() };
                let Some(resolver) = self.ui_icon_shader_resolver.as_mut() else { continue; };
                let fallback = if !is_extension &&
                    (ui::ICON_WEAPON_NO_AMMO_BASE..ui::ICON_INVENTORY_BASE).contains(&index) {
                    Some(if index >= ui::ICON_WEAPON_STAFF {
                        "gfx/hud/w_icon_lightsaber"
                    } else { name })
                } else { None };
                let materials = resolver.fx_material_stages_if_present(&requested)
                    .or_else(|| fallback.and_then(|path| resolver.fx_material_stages_if_present(path)));
                let Some(stages) = materials else {
                    eprintln!("HUD shader {requested}: unable to resolve authored material");
                    // Avoid repeating VFS lookups/log spam at 2000 FPS.
                    self.ui_icon_shader_materials.insert(index, Vec::new());
                    continue;
                };
                self.ui_icon_shader_materials.insert(index, stages);
            }
            // Temporarily move the cached stage list to avoid borrowing self
            // while lazy-created FX pipelines and texture bind groups are built.
            let Some(stages) = self.ui_icon_shader_materials.remove(&index) else { continue; };
            let shader_start = self.ui_icon_shader_vertices.len();
            for stage in stages.iter().take(64) {
                if self.ui_icon_shader_vertices.len() + quad.len() > 16_384 { break; }
                self.ensure_screen_fx_pipeline(stage.blend);
                let texture_key = stage.texture_at_time(seconds)
                    .map(|texture| self.ensure_screen_fx_texture(&texture));
                let time = jka_assets::shader::shader_time(
                    seconds, stage.shader_time_offset, stage.shader_clamp_time,
                );
                let start = self.ui_icon_shader_vertices.len() as u32;
                for mut v in quad.iter().copied() {
                    // The extension's atlas UVs encode clipping and mirroring.
                    // Remap them into the original image's 0..1 UV domain
                    // before applying authored tcMods and sampling stage maps.
                    if index >= ui::ICON_NAMES.len() {
                        let (uv0, uv1) = ui::extension_image_cell_uv_for_count(index - ui::ICON_NAMES.len(), self.ui_state.extension_images.len());
                        v.uv = [
                            (v.uv[0] - uv0[0]) / (uv1[0] - uv0[0]),
                            (v.uv[1] - uv0[1]) / (uv1[1] - uv0[1]),
                        ];
                    }
                    v.uv = stage.tc_mods.iter().fold(v.uv, |uv, &modifier|
                        crate::fx::draw::modified_uv(uv, [0.0; 3], modifier, time));
                    v.color = stage.float_vertex_color_at_time(v.color, seconds);
                    let cut = stage.alpha_test;
                    v.textured = if cut >= 0.74 { 3.0 } else if cut >= 0.49 { 2.0 }
                        else if cut > 0.0 { 4.0 } else if cut < 0.0 { 5.0 } else { 1.0 };
                    self.ui_icon_shader_vertices.push(v);
                }
                self.ui_icon_shader_batches.push(UiIconShaderBatch {
                    layer,
                    original_start,
                    original_end,
                    stage: ScreenFxBatch {
                        range: start..self.ui_icon_shader_vertices.len() as u32,
                        blend: stage.blend,
                        texture_key,
                    },
                    user: None,
                });
            }
            if shader_start == self.ui_icon_shader_vertices.len() {
                rverbose!(1, "HUD shader icon {index}: no renderable stages");
            }
            self.ui_icon_shader_materials.insert(index, stages);
        }
        if let Some(gpu) = self.hud_shader_gpu.as_ref() {
            if !gpu.pending.is_empty() {
                self.queue.write_buffer(&gpu.uniforms, 0, &gpu.pending);
            }
        }
        // Render with the already compiled pipeline, or display the original
        // image while a first-use shader PSO is being prepared asynchronously.
        let names: Vec<_> = self.ui_icon_shader_batches.iter()
            .filter_map(|batch| batch.user.as_ref().map(|(name, _)| name.clone()))
            .collect();
        // Prepare the native and encoded attachment variants independently.
        // Both remain cold until this extension actually draws an image shader.
        // The encoded target can be BGRA UNORM or HDR's RGBA UNORM UI canvas.
        let native_format = self.config.format;
        let encoded_format = self.native_2d_encoded_surface_eligible().then(|| {
            if self.hdr_output.is_some() {
                wgpu::TextureFormat::Rgba8Unorm
            } else {
                native_format.remove_srgb_suffix()
            }
        }).filter(|&format| format != native_format);
        for name in names {
            self.ensure_hud_shader_pipeline(&name, native_format);
            if let Some(format) = encoded_format {
                self.ensure_hud_shader_pipeline(&name, format);
            }
        }
        if !self.ui_icon_shader_vertices.is_empty() {
            self.queue.write_buffer(
                &self.ui_icon_shader_buffer, 0,
                bytemuck::cast_slice(&self.ui_icon_shader_vertices),
            );
        }
    }

    pub(in crate::renderer) fn draw_ui_batches<'a>(
        pass: &mut wgpu::RenderPass<'a>,
        ui_pipeline: &'a wgpu::RenderPipeline,
        ui_bind_group: &'a wgpu::BindGroup,
        dynamic_buffer: &'a wgpu::Buffer,
        dynamic_vertex_count: u32,
        transient_buffer: &'a wgpu::Buffer,
        transient_vertex_count: u32,
        retained_buffer: &'a wgpu::Buffer,
        retained_vertex_count: u32,
    ) {
        pass.set_pipeline(ui_pipeline);
        pass.set_bind_group(0, ui_bind_group, &[]);
        // Draw the gameplay HUD, transient chat/center-print and telemetry, then
        // retained foreground UI such as the scoreboard, console, and inspector.
        if dynamic_vertex_count != 0 {
            pass.set_vertex_buffer(0, dynamic_buffer.slice(..));
            pass.draw(0..dynamic_vertex_count, 0..1);
        }
        if transient_vertex_count != 0 {
            pass.set_vertex_buffer(0, transient_buffer.slice(..));
            pass.draw(0..transient_vertex_count, 0..1);
        }
        if retained_vertex_count != 0 {
            pass.set_vertex_buffer(0, retained_buffer.slice(..));
            pass.draw(0..retained_vertex_count, 0..1);
        }
    }

    /// Preserve painter order for mixed native atlas quads and authored FX
    /// stages. In the common case (no shader-tagged UI), call the old fast
    /// batcher directly: zero extra draws and no per-frame texture work.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::renderer) fn draw_ui_batches_shadered<'a>(
        pass: &mut wgpu::RenderPass<'a>,
        ui_pipeline: &'a wgpu::RenderPipeline,
        ui_group: &'a wgpu::BindGroup,
        ui_gamma_group: Option<&'a wgpu::BindGroup>,
        dynamic_buffer: &'a wgpu::Buffer,
        dynamic_count: u32,
        transient_buffer: &'a wgpu::Buffer,
        transient_count: u32,
        retained_buffer: &'a wgpu::Buffer,
        retained_count: u32,
        shader_buffer: &'a wgpu::Buffer,
        shader_batches: &'a [UiIconShaderBatch],
        shaders: &'a HashMap<FxBlend, wgpu::RenderPipeline>,
        textures: &'a HashMap<String, ScreenFxTexture>,
        white: &'a wgpu::BindGroup,
        encoded: bool,
        shader_target_format: wgpu::TextureFormat,
        user_gpu: Option<&'a HudShaderGpu>,
    ) {
        if shader_batches.is_empty() {
            Self::draw_ui_batches(pass, ui_pipeline, ui_group,
                dynamic_buffer, dynamic_count, transient_buffer,
                transient_count, retained_buffer, retained_count);
            return;
        }
        for (layer, (buffer, total)) in [
            (dynamic_buffer, dynamic_count),
            (transient_buffer, transient_count),
            (retained_buffer, retained_count),
        ].into_iter().enumerate() {
            let mut cursor = 0;
            let mut index = 0;
            while index < shader_batches.len() {
                let item = &shader_batches[index];
                if item.layer as usize != layer { index += 1; continue; }
                let start = item.original_start.min(total);
                let end = item.original_end.min(total);
                if start > cursor {
                    pass.set_pipeline(ui_pipeline);
                    pass.set_bind_group(0, ui_group, &[]);
                    if let Some(gamma) = ui_gamma_group {
                        pass.set_bind_group(1, gamma, &[]);
                    }
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    pass.draw(cursor..start, 0..1);
                }
                // All stages for this original span are emitted sequentially,
                // before the next native HUD element is drawn.
                pass.set_vertex_buffer(0, shader_buffer.slice(..));
                while index < shader_batches.len()
                    && shader_batches[index].layer as usize == layer
                    && shader_batches[index].original_start == item.original_start
                {
                    let batch = &shader_batches[index].stage;
                    if !batch.range.is_empty() {
                        if let Some((name, offset)) = &shader_batches[index].user {
                            // Do not draw the shader's source noise atlas
                            // image as a rectangular placeholder before its
                            // first-use PSO is ready. The extension's ordinary
                            // images, frame and text remain visible meanwhile.
                            if let Some(gpu) = user_gpu {
                                if let Some(pipeline) = gpu.pipelines.get(name)
                                    .and_then(|variants| variants.get(&shader_target_format)) {
                                    pass.set_pipeline(pipeline);
                                    pass.set_bind_group(0, ui_group, &[]);
                                    pass.set_bind_group(1, &gpu.group, &[*offset]);
                                    pass.draw(batch.range.clone(), 0..1);
                                }
                            }
                        } else if let Some(pipeline) = shaders.get(&batch.blend) {
                            let group = batch.texture_key.as_ref()
                                .and_then(|key| textures.get(key))
                                .map_or(white, |texture| if encoded { &texture.encoded_bind_group } else { &texture.bind_group });
                            pass.set_pipeline(pipeline);
                            pass.set_bind_group(0, group, &[]);
                            if encoded {
                                if let Some(gamma) = ui_gamma_group { pass.set_bind_group(1, gamma, &[]); }
                            }
                            pass.draw(batch.range.clone(), 0..1);
                        }
                    }
                    index += 1;
                }
                cursor = end;
            }
            if cursor < total {
                pass.set_pipeline(ui_pipeline);
                pass.set_bind_group(0, ui_group, &[]);
                if let Some(gamma) = ui_gamma_group {
                    pass.set_bind_group(1, gamma, &[]);
                }
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(cursor..total, 0..1);
            }
        }
    }

    pub(in crate::renderer) fn draw_screen_fx_batches<'a>(
        pass: &mut wgpu::RenderPass<'a>,
        vertex_buffer: &'a wgpu::Buffer,
        vertex_count: u32,
        batches: &'a [ScreenFxBatch],
        pipelines: &'a HashMap<FxBlend, wgpu::RenderPipeline>,
        textures: &'a HashMap<String, ScreenFxTexture>,
        white_bind_group: &'a wgpu::BindGroup,
        encoded: bool,
        encoded_gamma_group: Option<&'a wgpu::BindGroup>,
    ) {
        if vertex_count == 0 {
            return;
        }
        pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        for batch in batches {
            let Some(pipeline) = pipelines.get(&batch.blend) else {
                continue;
            };
            let bind_group = batch
                .texture_key
                .as_ref()
                .and_then(|key| textures.get(key))
                .map_or(white_bind_group, |texture| if encoded { &texture.encoded_bind_group } else { &texture.bind_group });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind_group, &[]);
            if let Some(gamma) = encoded_gamma_group { pass.set_bind_group(1, gamma, &[]); }
            pass.draw(batch.range.clone(), 0..1);
        }
    }

    /// With a menu open this runs *after* egui so the readout stays on top of
    /// the menu instead of being covered by it: these are status displays the
    /// player is usually watching while changing a setting.
    pub(in crate::renderer) fn submit_ui_overlay(&mut self, frame_view: &wgpu::TextureView, encoded_surface: Option<&wgpu::TextureView>) {
        self.prepare_enemy_icon_shader();
        if self.enemy_icon_vertex_count == 0 && self.screen_fx_vertex_count == 0
            && self.ui_vertex_count == 0
            && self.ui_dynamic_vertex_count == 0
            && self.ui_transient_vertex_count == 0
            && !(self.frame_time_graph.draw_ready() && !self.egui_active)
        {
            return;
        }
        let gamma_active = self.ui_gamma_active();
        let encoded = encoded_surface.is_some();
        let ui_gamma = if gamma_active { self.ui_gamma_native.as_ref() } else { None };
        let encoded_gamma = if encoded { ui_gamma } else { None };
        let encoded_gamma_group = encoded_gamma.map(|r| &r.encoded_group);
        let ui_view = encoded_surface.unwrap_or(frame_view);
        let ui_pipelines = if let Some(gamma) = encoded_gamma { &gamma.encoded_fx_pipelines }
            else if encoded { &self.screen_fx_encoded_pipelines } else { &self.screen_fx_pipelines };
        let ui_group = if encoded { &self.ui_encoded_bind_group } else { &self.ui_bind_group };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("JKA UI overlay encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("JKA UI overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: ui_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if encoded && self.hdr_output.is_some()
                            && (!self.egui_active || (self.egui_paint_jobs.is_empty() && !self.neon_chrome_wanted())) {
                            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            // OpenJK draws CG_SaberClashFlare in CG_Draw2D before the ordinary
            // HUD status elements. Keep that ordering and its authored shader
            // blend, while leaving retained UI/console on top.
            Self::draw_screen_fx_batches(
                &mut pass,
                &self.screen_fx_vertex_buffer,
                self.screen_fx_vertex_count,
                &self.screen_fx_batches,
                ui_pipelines,
                &self.screen_fx_textures,
                &self.screen_fx_white_bind_group, encoded, encoded_gamma_group,
            );

            // No extra scene pass: only the native UI pixels are counter-adjusted.
            let ui_pipeline = if let Some(gamma) = encoded_gamma {
                gamma.encoded_pipeline.as_ref().expect("ready encoded gamma UI pipeline")
            } else if encoded {
                self.ui_encoded_pipeline.as_ref().expect("ready encoded UI pipeline")
            } else {
                ui_gamma.map_or(&self.ui_pipeline, |r| &r.pipeline)
            };
            if let Some(group) = encoded_gamma_group.or_else(|| ui_gamma.map(|r| &r.group)) {
                pass.set_bind_group(1, group, &[]);
            }
            Self::draw_ui_batches_shadered(
                &mut pass,
                ui_pipeline,
                ui_group,
                encoded_gamma_group.or_else(|| ui_gamma.map(|r| &r.group)),
                &self.ui_dynamic_vertex_buffer,
                self.ui_dynamic_vertex_count,
                &self.ui_transient_vertex_buffer,
                self.ui_transient_vertex_count,
                &self.ui_vertex_buffer,
                self.ui_vertex_count,
                &self.ui_icon_shader_buffer,
                &self.ui_icon_shader_batches,
                ui_pipelines,
                &self.screen_fx_textures,
                &self.screen_fx_white_bind_group, encoded,
                if encoded {
                    self.ui_encoded_format.expect("encoded 2D UI pipeline initialized")
                } else {
                    self.config.format
                },
                self.hud_shader_gpu.as_ref(),
            );
            // The single enemy-info icon is a standalone authored multi-stage
            // shader. It never samples the 64x64 UI icon atlas.
            Self::draw_screen_fx_batches(
                &mut pass,
                &self.enemy_icon_vertex_buffer,
                self.enemy_icon_vertex_count,
                &self.enemy_icon_batches,
                ui_pipelines,
                &self.screen_fx_textures,
                &self.screen_fx_white_bind_group, encoded, encoded_gamma_group,
            );
            if !self.egui_active {
                self.frame_time_graph.draw(&mut pass, ui_pipeline, ui_group);
            }
        }
        self.queue.submit([encoder.finish()]);
    }

    pub(in crate::renderer) fn rebuild_ui(&mut self) {
        self.rebuild_retained_ui();
        self.rebuild_dynamic_ui();
        self.rebuild_transient_ui();
    }

    pub(in crate::renderer) fn rebuild_retained_ui(&mut self) {
        let trace_started = self.ui_trace.then(Instant::now);
        if self.ui_state.video.skip_ui {
            self.ui_vertex_count = 0;
            return;
        }
        let mut vertices = ui::build_vertices(
            &self.ui_state,
            self.config.width,
            self.config.height,
            self.ui_small_font_metrics.as_ref(),
            self.ui_splash_size,
        );
        let max_vertices = UI_BUFFER_BYTES as usize / std::mem::size_of::<UiVertex>();
        if vertices.len() > max_vertices {
            vertices.truncate(max_vertices - (max_vertices % 3));
        }
        self.ui_vertex_count = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
        if self.ui_vertex_count != 0 {
            self.queue
                .write_buffer(&self.ui_vertex_buffer, 0, bytemuck::cast_slice(&vertices));
        }
        if self.ui_state.icon_shader_flags.iter().any(|&authored| authored) {
            self.ui_retained_vertices = vertices;
        } else {
            self.ui_retained_vertices.clear();
        }
        if let Some(started) = trace_started {
            rverbose!(
                2,
                "[JKA UI] retained rebuild {:.3} ms, {} vertices ({} triangles)",
                started.elapsed().as_secs_f64() * 1000.0,
                self.ui_vertex_count,
                self.ui_vertex_count / 3
            );
        }
    }

    pub(in crate::renderer) fn rebuild_dynamic_ui(&mut self) {
        let trace_started = self.ui_trace.then(Instant::now);
        self.ui_dynamic_vertices.clear();
        self.ui_dynamic_stable_vertex_count = 0;
        if self.ui_state.video.skip_ui {
            self.ui_dynamic_vertex_count = 0;
            return;
        }

        // Vignette-only mode is a few quads in the same gameplay batch. Keeping
        // it in the stable prefix preserves the original painter order.
        if self.vignette_enabled && !self.frame_plan.use_post {
            append_vignette_vertices(&mut self.ui_dynamic_vertices);
        }
        ui::build_dynamic_static_vertices(
            &mut self.ui_dynamic_vertices,
            &self.ui_state,
            self.config.width,
            self.config.height,
        );
        let max_vertices = UI_DYNAMIC_BUFFER_BYTES as usize / std::mem::size_of::<UiVertex>();
        if self.ui_dynamic_vertices.len() > max_vertices {
            self.ui_dynamic_vertices
                .truncate(max_vertices - (max_vertices % 3));
        }
        self.ui_dynamic_stable_vertex_count = self.ui_dynamic_vertices.len();
        ui::build_movement_vertices(
            &mut self.ui_dynamic_vertices,
            &self.ui_state,
            self.config.width,
            self.config.height,
        );
        if self.ui_dynamic_vertices.len() > max_vertices {
            self.ui_dynamic_vertices
                .truncate(max_vertices - (max_vertices % 3));
        }
        self.ui_dynamic_vertex_count =
            u32::try_from(self.ui_dynamic_vertices.len()).unwrap_or(u32::MAX);
        if self.ui_dynamic_vertex_count != 0 {
            self.queue.write_buffer(
                &self.ui_dynamic_vertex_buffer,
                0,
                bytemuck::cast_slice(&self.ui_dynamic_vertices),
            );
        }
        if let Some(started) = trace_started {
            rverbose!(
                2,
                "[JKA UI] gameplay rebuild {:.3} ms, {} stable + {} hot vertices",
                started.elapsed().as_secs_f64() * 1000.0,
                self.ui_dynamic_stable_vertex_count,
                self.ui_dynamic_vertices
                    .len()
                    .saturating_sub(self.ui_dynamic_stable_vertex_count)
            );
        }
    }

    pub(in crate::renderer) fn rebuild_movement_ui(&mut self) {
        let trace_started = self.ui_trace.then(Instant::now);
        if self.ui_state.video.skip_ui {
            self.ui_dynamic_vertex_count = 0;
            return;
        }
        let max_vertices = UI_DYNAMIC_BUFFER_BYTES as usize / std::mem::size_of::<UiVertex>();
        let stable = self
            .ui_dynamic_stable_vertex_count
            .min(self.ui_dynamic_vertices.len())
            .min(max_vertices);
        self.ui_dynamic_vertices.truncate(stable);
        ui::build_movement_vertices(
            &mut self.ui_dynamic_vertices,
            &self.ui_state,
            self.config.width,
            self.config.height,
        );
        if self.ui_dynamic_vertices.len() > max_vertices {
            self.ui_dynamic_vertices
                .truncate(max_vertices - (max_vertices % 3));
        }
        self.ui_dynamic_vertex_count =
            u32::try_from(self.ui_dynamic_vertices.len()).unwrap_or(u32::MAX);

        // The stable prefix is already resident on the GPU. Upload only the tail
        // that changed with this input/simulation snapshot.
        let tail = &self.ui_dynamic_vertices[stable..];
        if !tail.is_empty() {
            let byte_offset = (stable * std::mem::size_of::<UiVertex>()) as u64;
            self.queue.write_buffer(
                &self.ui_dynamic_vertex_buffer,
                byte_offset,
                bytemuck::cast_slice(tail),
            );
        }
        if let Some(started) = trace_started {
            rverbose!(
                2,
                "[JKA UI] movement tail {:.3} ms, {} vertices / {} bytes",
                started.elapsed().as_secs_f64() * 1000.0,
                tail.len(),
                tail.len() * std::mem::size_of::<UiVertex>()
            );
        }
    }

    pub(in crate::renderer) fn rebuild_transient_ui(&mut self) {
        let trace_started = self.ui_trace.then(Instant::now);
        self.ui_transient_vertices.clear();
        if self.ui_state.video.skip_ui {
            self.ui_transient_vertex_count = 0;
            self.ui_transient_stable_vertex_count = 0;
            return;
        }
        ui::build_transient_vertices(
            &mut self.ui_transient_vertices,
            &self.ui_state,
            self.ui_small_font_metrics.as_ref(),
            self.config.width,
            self.config.height,
        );
        let max_vertices = UI_TRANSIENT_BUFFER_BYTES as usize / std::mem::size_of::<UiVertex>();
        if self.ui_transient_vertices.len() > max_vertices {
            self.ui_transient_vertices
                .truncate(max_vertices - (max_vertices % 3));
        }
        self.ui_transient_stable_vertex_count = self.ui_transient_vertices.len();
        self.ui_transient_vertex_count =
            u32::try_from(self.ui_transient_vertices.len()).unwrap_or(u32::MAX);
        // The stable part changed, so the tail's resident copy is no longer known.
        self.ui_transient_tail_uploaded = None;
        if self.ui_transient_vertex_count != 0 {
            self.queue.write_buffer(
                &self.ui_transient_vertex_buffer,
                0,
                bytemuck::cast_slice(&self.ui_transient_vertices),
            );
        }
        if let Some(started) = trace_started {
            rverbose!(
                2,
                "[JKA UI] transient rebuild {:.3} ms, {} vertices",
                started.elapsed().as_secs_f64() * 1000.0,
                self.ui_transient_vertex_count
            );
        }
    }

    pub(in crate::renderer) fn rebuild_player_name_tail(
        &mut self,
        view_proj: Mat4,
        dynamic_crosshair_world: Option<[f32; 3]>,
    ) {
        self.ui_transient_vertices
            .truncate(self.ui_transient_stable_vertex_count);
        if self.ui_state.video.skip_ui {
            self.ui_transient_vertex_count = 0;
            return;
        }

        // TaystJK keeps the crosshair hit in world space until draw time. Do the
        // same here, after the renderer samples the newest subframe view. The endpoint
        // and aim angles arrive as one LatestViewState sample, so this projection uses
        // the exact camera that produced that hit rather than a previous-frame point.
        let tail_start = self.ui_transient_vertices.len();
        // Projected player names and the late-latched crosshair are screen-space
        // CG_Draw2D, even though their vertices are appended after HUD publishing.
        // Check the snapshot boolean once; no cvar lookup in the render loop.
        if self.ui_state.video.draw_2d {
            // Both effects are cg_draw.c 2D output; the rocket target must be
            // projected with the final late-latched view, not an earlier pose.
            let rocket_screen = self.ui_state.rocket_lock.and_then(|lock| {
                let clip = view_proj * Vec3::from_array(crate::scene::render_position(lock.target)).extend(1.0);
                if clip.w <= 0.001 { return None; }
                let ndc = clip.truncate() / clip.w;
                ((0.0..=1.0).contains(&ndc.z) && ndc.x.abs() <= 1.15 && ndc.y.abs() <= 1.15)
                    .then(|| [(ndc.x * 0.5 + 0.5) * self.config.width.max(1) as f32,
                              (0.5 - ndc.y * 0.5) * self.config.height.max(1) as f32])
            });
            ui::build_combat_hud_vertices(
                &mut self.ui_transient_vertices, &self.ui_state, rocket_screen,
                self.config.width, self.config.height,
            );
            if ui::gameplay_hud_visible(&self.ui_state, self.config.width, self.config.height) {
                if let Some(anchor) = dynamic_crosshair_world {
                    let clip = view_proj * Vec3::from_array(anchor).extend(1.0);
                    if clip.w > 0.001 {
                        let ndc = clip.truncate() / clip.w;
                        if (0.0..=1.0).contains(&ndc.z) && ndc.x.abs() <= 1.15 && ndc.y.abs() <= 1.15 {
                            let x = (ndc.x * 0.5 + 0.5) * self.config.width.max(1) as f32;
                            let y = (0.5 - ndc.y * 0.5) * self.config.height.max(1) as f32;
                            ui::build_crosshair_vertices(
                                &mut self.ui_transient_vertices,
                                &self.ui_state,
                                Some([x, y]),
                                self.config.width,
                                self.config.height,
                            );
                        }
                    }
                }
            }

            if let Some(player_names) = self.player_names.as_ref() {
                // Unlike the legacy 640x480 projection, these markers follow
                // the same late-latched, aspect-correct view used by the models.
                // Off/restricted: leads is empty; no extra per-frame work.
                for lead in player_names.leads.iter() {
                    let project = |position: [f32; 3]| {
                        let clip = view_proj * Vec3::from_array(position).extend(1.0);
                        if clip.w <= 0.001 { return None; }
                        let ndc = clip.truncate() / clip.w;
                        if !(0.0..=1.0).contains(&ndc.z) { return None; }
                        Some([(ndc.x * 0.5 + 0.5) * self.config.width.max(1) as f32,
                              (0.5 - ndc.y * 0.5) * self.config.height.max(1) as f32])
                    };
                    let Some(current) = project(lead.current) else { continue; };
                    if current[0] < 0.0 || current[0] >= self.config.width as f32
                        || current[1] < 0.0 || current[1] >= self.config.height as f32 { continue; }
                    let Some(predicted) = project(lead.predicted) else { continue; };
                    ui::build_lead_indicator_vertices(&mut self.ui_transient_vertices,
                        current, predicted, self.config.width, self.config.height);
                }
                for label in player_names.labels.iter() {
                    let clip = view_proj * Vec3::from_array(label.anchor).extend(1.0);
                    if clip.w <= 0.001 {
                        continue;
                    }
                    let ndc = clip.truncate() / clip.w;
                    if !(0.0..=1.0).contains(&ndc.z) || ndc.x.abs() > 1.15 || ndc.y.abs() > 1.15 {
                        continue;
                    }
                    let x = (ndc.x * 0.5 + 0.5) * self.config.width.max(1) as f32;
                    let y = (0.5 - ndc.y * 0.5) * self.config.height.max(1) as f32;
                    ui::build_world_player_label_vertices(
                        &mut self.ui_transient_vertices,
                        label,
                        x,
                        y,
                        player_names.scale,
                        self.ui_small_font_metrics.as_ref(),
                        self.config.width,
                        self.config.height,
                    );
                }
            }
        }

        let max_vertices = UI_TRANSIENT_BUFFER_BYTES as usize / std::mem::size_of::<UiVertex>();
        if self.ui_transient_vertices.len() > max_vertices {
            self.ui_transient_vertices
                .truncate(max_vertices - (max_vertices % 3));
        }
        self.ui_transient_vertex_count =
            u32::try_from(self.ui_transient_vertices.len()).unwrap_or(u32::MAX);
        self.prepare_ui_icon_shaders();
        let tail = &self.ui_transient_vertices[tail_start.min(self.ui_transient_vertices.len())..];
        if !tail.is_empty() {
            let byte_offset = (tail_start * std::mem::size_of::<UiVertex>()) as u64;
            let bytes: &[u8] = bytemuck::cast_slice(tail);
            // A static crosshair re-sends the same vertices every frame; each
            // queue write is its own GPU copy (~9 us on a UHD 630).
            let unchanged = self.ui_transient_tail_uploaded.as_ref()
                .is_some_and(|(offset, uploaded)| *offset == byte_offset && uploaded.as_slice() == bytes);
            if !unchanged {
                self.queue.write_buffer(&self.ui_transient_vertex_buffer, byte_offset, bytes);
                let (offset, uploaded) = self.ui_transient_tail_uploaded
                    .get_or_insert_with(|| (byte_offset, Vec::new()));
                *offset = byte_offset;
                uploaded.clear();
                uploaded.extend_from_slice(bytes);
            }
        }
    }
}

/// Place images on shelves, largest-first. Each icon gets a one-texel
/// extruded border, which gives linear filtering the same edge clamp as a
/// standalone GL texture. No fixed 64/256 cell or resampling is involved.
fn pack_native_icon_shelves(
    images: &[Option<image::RgbaImage>],
    width: u32,
    max_height: u32,
) -> Option<(u32, Vec<(u32, u32)>)> {
    let mut order: Vec<usize> = (0..images.len())
        .filter(|&i| images[i].is_some())
        .collect();
    order.sort_unstable_by_key(|&i| {
        let im = images[i].as_ref().unwrap();
        (std::cmp::Reverse(im.height()), std::cmp::Reverse(im.width()), i)
    });
    let mut coords = vec![(0, 0); images.len()];
    let mut x: u32 = 2;
    let mut y: u32 = 2;
    let mut row_height: u32 = 0;
    for i in order {
        let image = images[i].as_ref().unwrap();
        let (rw, rh) = (image.width() + 2, image.height() + 2);
        if rw + 4 > width || rh + 4 > max_height { return None; }
        if x + rw + 2 > width {
            y += row_height;
            x = 2;
            row_height = 0;
        }
        if y + rh + 2 > max_height { return None; }
        coords[i] = (x + 1, y + 1);
        x += rw;
        row_height = row_height.max(rh);
    }
    Some(((y + row_height + 2).max(4), coords))
}

/// Keep image-only HUD icons at their decoded native width and height. Authored
/// shader materials bypass this atlas entirely, preserving their live stages.
/// Only shrink an
/// exceptional oversized image when GPU texture-size or memory limits force it;
/// unlike the old fixed-cell atlas, ordinary 32px, 128px, 512px and HD art
/// are copied byte-for-byte into this texture. Returns both pixels and per-icon
/// UVs so weapons, Force icons, crosshairs and the lagometer share one binding.
fn assemble_native_icon_atlas(
    mut images: Vec<Option<image::RgbaImage>>,
    max_texture_size: u32,
) -> (TextureData, Vec<([f32; 2], [f32; 2])>) {
    // Cap to a broadly supported size and to 128MiB of uncompressed atlas
    // memory. Individual images remain native unless packing cannot fit.
    let limit = max_texture_size.min(8192).max(8);
    const MAX_PIXELS: u64 = 32 * 1024 * 1024;
    let mut reductions = 0;
    for (i, opt) in images.iter_mut().enumerate() {
        let Some(image) = opt.as_mut() else { continue; };
        let (w, h) = image.dimensions();
        if w + 6 <= limit && h + 6 <= limit { continue; }
        let factor = ((limit - 6) as f64 / w as f64)
            .min((limit - 6) as f64 / h as f64)
            .min(1.0);
        let (nw, nh) = (((w as f64 * factor).floor() as u32).max(1),
                        ((h as f64 * factor).floor() as u32).max(1));
        eprintln!("UI icon {}: {}x{} exceeds GPU atlas limit, shrinking to {}x{}", i, w, h, nw, nh);
        *image = image::imageops::resize(&*image, nw, nh, image::imageops::FilterType::Lanczos3);
        reductions += 1;
    }
    let (width, height, positions) = loop {
        let image_area = images.iter().flatten().map(|im| {
            u64::from(im.width() + 2) * u64::from(im.height() + 2)
        }).sum::<u64>();
        let widest = images.iter().flatten().map(|im| im.width() + 6).max().unwrap_or(4);
        let desired = (image_area as f64).sqrt().ceil() as u32;
        let mut width = desired.max(widest).max(4).next_power_of_two().min(limit);
        let mut packed = None;
        loop {
            if let Some((height, positions)) = pack_native_icon_shelves(&images, width, limit) {
                if u64::from(width) * u64::from(height) <= MAX_PIXELS {
                    packed = Some((width, height, positions));
                    break;
                }
            }
            if width == limit { break; }
            width = width.saturating_mul(2).min(limit);
        }
        if let Some(layout) = packed { break layout; }
        // Real texture constraints, not an arbitrary icon-size target, are the
        // only reason we ever resample. Reduce the largest icon first.
        let Some((i, _)) = images.iter().enumerate().filter_map(|(i, im)| {
            im.as_ref().map(|im| (i, u64::from(im.width()) * u64::from(im.height())))
        }).filter(|&(_, area)| area > 1).max_by_key(|&(_, area)| area) else {
            eprintln!("UI icon atlas: cannot pack icons within texture limits; using transparent placeholder");
            break (4, 4, vec![(0, 0); images.len()]);
        };
        let im = images[i].as_ref().unwrap();
        let (w, h) = im.dimensions();
        let (nw, nh) = ((w * 3 / 4).max(1), (h * 3 / 4).max(1));
        eprintln!("UI icon {}: atlas capacity exceeded, shrinking {}x{} to {}x{}", i, w, h, nw, nh);
        images[i] = Some(image::imageops::resize(im, nw, nh, image::imageops::FilterType::Lanczos3));
        reductions += 1;
    };

    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    let mut uvs = vec![([0.5 / width as f32, 0.5 / height as f32],
                        [0.5 / width as f32, 0.5 / height as f32]); images.len()];
    for (i, opt) in images.iter().enumerate() {
        let Some(im) = opt.as_ref() else { continue; };
        let (x, y) = positions[i];
        if x == 0 || y == 0 { continue; }
        let (w, h) = im.dimensions();
        // Duplicate the outside texel into a one-pixel gutter on all four
        // sides. Bilinear filtering can't bleed from neighbouring artwork.
        for dy in 0..(h + 2) {
            for dx in 0..(w + 2) {
                let src_x = dx.saturating_sub(1).min(w - 1);
                let src_y = dy.saturating_sub(1).min(h - 1);
                let px = im.get_pixel(src_x, src_y);
                let offset = (((y - 1 + dy) * width + (x - 1 + dx)) * 4) as usize;
                rgba[offset..offset + 4].copy_from_slice(&px.0);
            }
        }
        uvs[i] = ([x as f32 / width as f32, y as f32 / height as f32],
                  [(x + w) as f32 / width as f32, (y + h) as f32 / height as f32]);
    }
    rverbose!(1, "UI icon atlas: {}x{}, {} native-size icons, {} GPU-limit reductions",
        width, height, images.iter().flatten().count(), reductions);
    (TextureData {
        label: "JKA UI native-resolution icon atlas".into(),
        source: None, width, height, rgba: (rgba).into(), rgba16f: None,
        mip_level_count: 1, clamp: true, srgb: true,
    }, uvs)
}

/// An explicitly declared shader is authoritative even if a texture with
/// exactly the same qpath also exists (R_RegisterShader semantics).
fn ui_icon_has_authored_shader(
    shaders: &std::collections::BTreeMap<String, jka_assets::shader::Shader>,
    qpath: &str,
) -> bool {
    jka_assets::shader::find_shader(shaders, qpath).is_some()
}

pub(in crate::renderer) fn load_ui_icon_atlas(
    base: &Path,
    game: Option<&Path>,
    max_texture_size: u32,
) -> (TextureData, Vec<([f32; 2], [f32; 2])>, Vec<bool>) {
    let mut images = vec![None; ui::ICON_NAMES.len()];
    let mut shaders = vec![false; ui::ICON_NAMES.len()];
    match AssetSearchPath::open_game(base, game) {
        Ok(mut assets) => {
            // Use the same shader-library parsing/override precedence as the
            // world and model loader (including .oshader), not a filename-only
            // atlas lookup. Resolve once per atlas creation, never per frame.
            let mut shader_warnings = Vec::new();
            let shader_library = match crate::materials::shader_library(
                &mut assets, &mut shader_warnings, false,
            ) {
                Ok((library, _diagnostics)) => library,
                Err(error) => {
                    eprintln!("UI icon shader library: {error}");
                    let (texture, uvs) = assemble_native_icon_atlas(images, max_texture_size);
                    return (texture, uvs, shaders);
                }
            };
            for warning in shader_warnings {
                rverbose!(2, "UI icon shader library: {warning}");
            }
            for index in 0..ui::ICON_NAMES.len() {
                let name = ui::icon_name(index);
                let name = name.as_ref();
                if name.is_empty() { continue; }
                let requested = if (ui::ICON_WEAPON_NO_AMMO_BASE..ui::ICON_WEAPON_STAFF).contains(&index) {
                    format!("{name}_na")
                } else {
                    (*name).to_owned()
                };
                let has_authored_shader = ui_icon_has_authored_shader(&shader_library, &requested);
                shaders[index] = has_authored_shader;
                // Authored shader definitions ALWAYS win, and all their stages
                // are rendered standalone. Only implicit images enter the atlas.
                let mut image = if has_authored_shader { None } else {
                    try_load_texture_asset_from_search_path(
                        &mut assets, &requested, true, false, true, "UI icon",
                    )
                };
                // TaystJK's missing no-ammo icons can fall back to the normal
                // weapon artwork, but never override a *found* authored shader.
                if image.is_none() && !has_authored_shader
                    && (ui::ICON_WEAPON_NO_AMMO_BASE..ui::ICON_INVENTORY_BASE).contains(&index)
                {
                    let fallback = if index >= ui::ICON_WEAPON_STAFF {
                        "gfx/hud/w_icon_lightsaber"
                    } else { name };
                    if jka_assets::shader::find_shader(&shader_library, fallback).is_some() {
                        shaders[index] = true;
                    } else {
                        image = try_load_texture_asset_from_search_path(
                            &mut assets, fallback, true, false, true, "UI icon fallback",
                        );
                    }
                }
                let Some(image) = image else {
                    if !shaders[index] { eprintln!("UI icon {requested}: not found"); }
                    continue;
                };
                images[index] = image::RgbaImage::from_raw(image.width, image.height, image.rgba.into_vec());
                if images[index].is_none() {
                    eprintln!("UI icon {requested}: unexpected pixel format");
                }
            }
        }
        Err(error) => eprintln!("UI icon asset search: {error}"),
    }
    let (texture, uvs) = assemble_native_icon_atlas(images, max_texture_size);
    (texture, uvs, shaders)
}

#[cfg(test)]
mod native_icon_atlas_tests {
    use super::*;

    #[test]
    fn authored_shader_ignores_colliding_image_and_marks_full_uvs() {
        let dir = std::env::temp_dir().join(format!("djk_ui_shader_{}", std::process::id()));
        let image_dir = dir.join("gfx/2d");
        std::fs::create_dir_all(&image_dir).unwrap();
        image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]))
            .save(image_dir.join("binocularWindow.png")).unwrap();
        let mut assets = AssetSearchPath::open(&dir).unwrap();
        assert!(try_load_texture_asset_from_search_path(
            &mut assets, "gfx/2d/binocularWindow", true, false, true, "test"
        ).is_some());
        let scripts = jka_assets::shader::parse(
            "gfx/2d/binocularWindow {\n{\nanimMap 5 gfx/2d/first gfx/2d/second\nblendFunc blend\n}\n}\n"
        ).unwrap();
        assert!(ui_icon_has_authored_shader(&scripts, "gfx/2d/binocularWindow"));
        let mut ui = ui::UiSnapshot::default();
        ui.icon_atlas_uvs = vec![([0.1, 0.1], [0.8, 0.8]); ui::ICON_NAMES.len()];
        ui.icon_shader_flags = vec![false; ui::ICON_NAMES.len()];
        let index = ui::ICON_NAMES.iter().position(|name| *name == "gfx/2d/binocularWindow").unwrap();
        ui.icon_shader_flags[index] = ui_icon_has_authored_shader(&scripts, "gfx/2d/binocularWindow");
        assert_eq!(ui.icon_uv(index), ([0.0, 0.0], [1.0, 1.0]));
        assert_eq!(ui_shader_icon_index(ui.icon_texture_source(index)), Some(index));
        assert_eq!(ui.icon_texture_source(0), 5.0);
        drop(assets);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn different_resolution_icons_are_copied_exactly_without_resampling() {
        let sizes = [(32, 16), (512, 256), (93, 71), (64, 64)];
        let mut icons = vec![None; sizes.len() + 1];
        for (i, (w, h)) in sizes.iter().copied().enumerate() {
            icons[i] = Some(image::RgbaImage::from_fn(w, h, |x, y| {
                image::Rgba([i as u8 * 13 + (x % 37) as u8,
                             (y % 251) as u8, (x % 253) as u8, 255])
            }));
        }
        let (atlas, uvs) = assemble_native_icon_atlas(icons.clone(), 2048);
        assert_eq!(uvs.len(), icons.len());
        assert_eq!(atlas.mip_level_count, 1);
        assert!(atlas.width <= 2048 && atlas.height <= 2048);
        for (i, icon) in icons.iter().enumerate() {
            let Some(im) = icon.as_ref() else { continue; };
            let (uv0, uv1) = uvs[i];
            let (x, y) = ((uv0[0] * atlas.width as f32).round() as u32,
                          (uv0[1] * atlas.height as f32).round() as u32);
            assert_eq!(((uv1[0] - uv0[0]) * atlas.width as f32).round() as u32, im.width());
            assert_eq!(((uv1[1] - uv0[1]) * atlas.height as f32).round() as u32, im.height());
            for yy in 0..im.height() {
                for xx in 0..im.width() {
                    let o = (((y + yy) * atlas.width + x + xx) * 4) as usize;
                    assert_eq!(&atlas.rgba[o..o + 4], &im.get_pixel(xx, yy).0[..]);
                }
            }
            // The extruded border is an exact copy of the first texel.
            let gutter = (((y - 1) * atlas.width + x - 1) * 4) as usize;
            assert_eq!(&atlas.rgba[gutter..gutter + 4], &im.get_pixel(0, 0).0[..]);
        }
        assert_eq!(uvs[icons.len() - 1].0, uvs[icons.len() - 1].1);
    }

    #[test]
    fn scales_only_when_hardware_constraints_prevent_native_packing() {
        let images = vec![Some(image::RgbaImage::new(256, 128))];
        let (atlas, uvs) = assemble_native_icon_atlas(images, 128);
        let size = ((uvs[0].1[0] - uvs[0].0[0]) * atlas.width as f32).round() as u32;
        assert!(size < 256);
        assert!(atlas.width <= 128 && atlas.height <= 128);
    }
}

/// Atlas capacity in rows, with the 32-image Rhai budget unchanged.
/// Native artwork uses additional rows only while a native HUD requests it.
fn ui_extension_atlas_rows(wanted_count: usize) -> u32 {
    (wanted_count.max(crate::extensions::MAX_HUD_IMAGES + 1) as u32)
        .div_ceil(ui::EXT_IMAGE_COLUMNS)
}

pub(in crate::renderer) fn blank_ui_extension_atlas() -> TextureData {
    blank_ui_extension_atlas_for_count(crate::extensions::MAX_HUD_IMAGES)
}

pub(in crate::renderer) fn blank_ui_extension_atlas_for_count(wanted_count: usize) -> TextureData {
    let cell = ui::EXT_IMAGE_CELL;
    let rows = ui_extension_atlas_rows(wanted_count);
    let (width, height) = (ui::EXT_IMAGE_COLUMNS * cell, rows * cell);
    TextureData {
        label: "JKA UI extension image atlas".into(),
        source: None,
        width,
        height,
        rgba: (vec![0u8; (width * height * 4) as usize]).into(),
        rgba16f: None,
        mip_level_count: 1,
        clamp: true,
        srgb: true,
    }
}

/// Decode extension `hud_image()` pictures into `ui::EXT_IMAGE_CELL` squares
/// (RGBA8, `None` when the image is missing). Images are stretched to the cell and
/// drawn with the whole cell as UVs, so their aspect ratio survives.
pub(in crate::renderer) fn load_ui_extension_images(
    base: &Path,
    game: Option<&Path>,
    names: &[String],
) -> Vec<Option<Vec<u8>>> {
    load_ui_extension_images_with_shaders(base, game, names).0
}

/// RegisterShader semantics: explicit shader definitions take precedence over
/// same-named bitmaps. Unauthored images still use the one-draw atlas path.
/// This work runs only when extension image registrations change, not per present.
pub(in crate::renderer) fn load_ui_extension_images_with_shaders(
    base: &Path,
    game: Option<&Path>,
    names: &[String],
) -> (Vec<Option<Vec<u8>>>, Vec<bool>) {
    let mut assets = match AssetSearchPath::open_game(base, game) {
        Ok(assets) => assets,
        Err(error) => {
            eprintln!("Extension HUD image asset search: {error}");
            return (vec![None; names.len()], vec![false; names.len()]);
        }
    };
    let mut warnings = Vec::new();
    let library = match crate::materials::shader_library(&mut assets, &mut warnings, false) {
        Ok((library, _)) => Some(library),
        Err(err) => { eprintln!("Extension HUD shader library: {err}"); None }
    };
    for warning in warnings { rverbose!(2, "Extension HUD shader library: {warning}"); }
    let (pixels, flags, _) = decode_ui_extension_image_cells(&mut assets, library.as_ref(), names);
    (pixels, flags)
}

/// Shared pure CPU decoding path, for the async loader and unit tests. The
/// caller supplies a shader library parsed once for its active asset mount.
fn decode_ui_extension_image_cells(
    assets: &mut AssetSearchPath,
    library: Option<&std::collections::BTreeMap<String, jka_assets::shader::Shader>>,
    names: &[String],
) -> (Vec<Option<Vec<u8>>>, Vec<bool>, Vec<Option<[u32; 2]>>) {
    let cell = ui::EXT_IMAGE_CELL;
    let flags: Vec<bool> = names.iter().map(|name| library.is_some_and(|library|
        ui_icon_has_authored_shader(library, name))).collect();
    let decoded: Vec<_> = names
        .iter().zip(flags.iter())
        .map(|(name, &authored)| {
            if authored { return (None, None); } // Authored materials use standalone stages.
            let Some(image) = try_load_texture_asset_from_search_path(
                assets, name, true, false, true, "extension HUD image",
            ) else {
                eprintln!("Extension HUD image {name}: not found");
                return (None, None);
            };
            let (source_w, source_h) = (image.width, image.height);
            let Some(source) = image::RgbaImage::from_raw(source_w, source_h, image.rgba.into_vec()) else {
                eprintln!("Extension HUD image {name}: unexpected pixel format");
                return (None, None);
            };
            let pixels = if source_w == cell && source_h == cell {
                source.into_raw()
            } else {
                image::imageops::resize(&source, cell, cell, image::imageops::FilterType::Triangle)
                    .into_raw()
            };
            (Some(pixels), Some([source_w, source_h]))
        })
        .collect();
    let (pixels, dimensions): (Vec<_>, Vec<_>) = decoded.into_iter().unzip();
    (pixels, flags, dimensions)
}

/// Dynamic atlas for CG_DrawTeamOverlay model, weapon and powerup icons.
/// Each cell key may contain newline-separated candidate qpaths; the first
/// existing image wins (team-skin icon -> team fallback -> defer art).
/// Every player icon the game can see (`models/players/<model>/icon_<skin>`,
/// every model and every skin, stock and pk3 alike), at 48 px, in one atlas
/// keyed `model/skin`. Runs off the render thread (see `sync_ui_avatars`).
pub(in crate::renderer) fn load_ui_avatar_atlas(base: &Path, game: Option<&Path>) -> (TextureData, ui::AvatarAtlas) {
    const CELL: u32 = 48;
    const COLUMNS: u32 = 128;
    // 8192 px tall at most: inside every adapter's minimum texture limit.
    const MAX_ICONS: usize = (COLUMNS * (8192 / CELL)) as usize;
    let mut icons: Vec<String> = Vec::new();
    let mut assets = AssetSearchPath::open_game(base, game).ok();
    if let Some(assets) = assets.as_ref() {
        for name in assets.names() {
            let lower = name.to_ascii_lowercase();
            let Some(rest) = lower.strip_prefix("models/players/") else { continue };
            let Some((model, file)) = rest.split_once('/') else { continue };
            let Some(stem) = file.strip_suffix(".tga").or_else(|| file.strip_suffix(".jpg")).or_else(|| file.strip_suffix(".png")) else {
                continue;
            };
            let Some(skin) = stem.strip_prefix("icon_") else { continue };
            if !skin.is_empty() && !model.contains('/') {
                icons.push(format!("{model}/{skin}"));
            }
        }
    }
    icons.sort();
    icons.dedup();
    if icons.len() > MAX_ICONS {
        // Keep every model's default icon before extra skins.
        icons.sort_by_key(|key| !key.ends_with("/default"));
        icons.truncate(MAX_ICONS);
    }
    let rows = (icons.len() as u32).div_ceil(COLUMNS).max(1);
    let (width, height) = (COLUMNS * CELL, rows * CELL);
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    let mut atlas = ui::AvatarAtlas { columns: COLUMNS, rows, cells: Default::default() };
    if let Some(assets) = assets.as_mut() {
        for key in icons {
            let Some((model, skin)) = key.split_once('/') else { continue };
            let qpath = format!("models/players/{model}/icon_{skin}");
            let Some(image) = try_load_texture_asset_from_search_path(assets, &qpath, true, false, true, "avatar") else {
                continue;
            };
            let Some(source) = image::RgbaImage::from_raw(image.width, image.height, image.rgba.into_vec()) else {
                continue;
            };
            let cell_image = if source.width() == CELL && source.height() == CELL {
                source
            } else {
                image::imageops::resize(&source, CELL, CELL, image::imageops::FilterType::Triangle)
            };
            let index = atlas.cells.len() as u32;
            let (col, row) = (index % COLUMNS, index / COLUMNS);
            let row_bytes = (CELL * 4) as usize;
            for y in 0..CELL {
                let src = y as usize * row_bytes;
                let dst = ((((row * CELL + y) * width) + col * CELL) * 4) as usize;
                rgba[dst..dst + row_bytes].copy_from_slice(&cell_image.as_raw()[src..src + row_bytes]);
            }
            atlas.cells.insert(key, index);
        }
    }
    let data = TextureData {
        label: "JKA UI avatar atlas".into(),
        source: None,
        width,
        height,
        rgba: rgba.into(),
        rgba16f: None,
        mip_level_count: 1,
        clamp: true,
        srgb: true,
    };
    (data, atlas)
}

pub(in crate::renderer) fn load_ui_team_icon_atlas(
    base: &Path,
    game: Option<&Path>,
    keys: &[String],
) -> TextureData {
    let cell = ui::TEAM_ICON_CELL;
    let columns = ui::TEAM_ICON_COLUMNS.max(1);
    let rows = ((keys.len() as u32 + columns - 1) / columns).max(1);
    let (width, height) = (columns * cell, rows * cell);
    let mut rgba = vec![0u8; (width * height * 4) as usize];

    match AssetSearchPath::open_game(base, game) {
        Ok(mut assets) => {
            for (index, key) in keys.iter().enumerate() {
                let mut loaded = None;
                for name in key.split('\n').filter(|name| !name.is_empty()) {
                    let resolved;
                    let qpath = if let Some(spec) = name.strip_prefix("@team-model-icon\t") {
                        let mut fields = spec.splitn(5, '\t');
                        let (
                            Some(model),
                            Some(skin),
                            Some(team),
                            Some(gametype),
                            Some(jedi_v_merc),
                        ) = (
                            fields.next(),
                            fields.next(),
                            fields.next(),
                            fields.next(),
                            fields.next(),
                        )
                        else {
                            continue;
                        };
                        let (Ok(team), Ok(gametype)) =
                            (team.parse::<i32>(), gametype.parse::<i32>())
                        else {
                            continue;
                        };
                        let mut final_skin = skin.to_owned();
                        if gametype >= crate::cgame::GT_TEAM
                            && gametype != crate::cgame::GT_SIEGE
                            && jedi_v_merc != "1"
                        {
                            let _ = crate::cgame::validate_skin_for_team(
                                model,
                                &mut final_skin,
                                team,
                                |candidate| assets.contains_qpath(candidate),
                            );
                        }
                        // TaystJK CG_LoadClientInfo registers the exact skin icon
                        // first. Only a composite (head|torso|legs) skin gets a
                        // second chance with the head icon. Do not invent a
                        // placeholder for CG_DrawEnemyInfo when neither exists.
                        let full_icon = format!("models/players/{model}/icon_{final_skin}");
                        if let Some(image) = try_load_texture_asset_from_search_path(
                            &mut assets,
                            &full_icon,
                            true,
                            false,
                            true,
                            "team overlay icon",
                        ) {
                            loaded = Some(image);
                            break;
                        }
                        if let Some((head, _)) = final_skin.split_once('|') {
                            resolved = format!("models/players/{model}/icon_{head}");
                            resolved.as_str()
                        } else {
                            continue;
                        }
                    } else {
                        name
                    };
                    if let Some(image) = try_load_texture_asset_from_search_path(
                        &mut assets,
                        qpath,
                        true,
                        false,
                        true,
                        "team overlay icon",
                    ) {
                        loaded = Some(image);
                        break;
                    }
                }
                let Some(image) = loaded else { continue };
                let (source_w, source_h) = (image.width, image.height);
                let Some(source) = image::RgbaImage::from_raw(source_w, source_h, image.rgba.into_vec())
                else {
                    continue;
                };
                let cell_image = if source_w == cell && source_h == cell {
                    source
                } else {
                    image::imageops::resize(
                        &source,
                        cell,
                        cell,
                        image::imageops::FilterType::Triangle,
                    )
                };
                let col = index as u32 % columns;
                let row = index as u32 / columns;
                let pixels = cell_image.as_raw();
                let row_bytes = (cell * 4) as usize;
                for y in 0..cell {
                    let src = y as usize * row_bytes;
                    let dst = ((((row * cell + y) * width) + col * cell) * 4) as usize;
                    rgba[dst..dst + row_bytes].copy_from_slice(&pixels[src..src + row_bytes]);
                }
            }
        }
        Err(error) => eprintln!("UI team overlay asset search: {error}"),
    }
    TextureData {
        label: "JKA UI team overlay atlas".into(),
        source: None,
        width,
        height,
        rgba: (rgba).into(),
        rgba16f: None,
        mip_level_count: 1,
        clamp: true,
        srgb: true,
    }
}

pub(in crate::renderer) fn load_ui_font_texture(base: &Path, game: Option<&Path>) -> TextureData {
    match AssetSearchPath::open_game(base, game) {
        Ok(mut assets) => {
            // Jedi Academy's native fixed charset. It is a 16x16 logical grid,
            // but OpenJK samples only the left half of each horizontal slot
            // (8x16 glyphs on a 16x16 pitch); ui::glyph_quad mirrors that layout.
            if let Some(data) = try_load_ui_font_asset(&mut assets, "gfx/2d/charsgrid_med") {
                return data;
            }
        }
        Err(error) => eprintln!("UI font asset search: {error}"),
    }

    let (width, height, rgba) = ui::fallback_font_rgba();
    rverbose!(1, "UI font: using built-in fallback charset");
    TextureData {
        label: "JKA UI fallback charset".into(),
        source: None,
        width,
        height,
        rgba: (rgba).into(),
        rgba16f: None,
        mip_level_count: 1,
        clamp: true,
        srgb: false,
    }
}

pub(in crate::renderer) fn loading_levelshot_stem(map_name: &str) -> Option<String> {
    let trimmed = map_name.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Keep map subdirectories intact (e.g. mp/ffa3 -> levelshots/mp/ffa3).
    // Only remove the map-source extension; Path::file_stem would incorrectly
    // collapse mp/ffa3.bsp to just ffa3.
    let lower = trimmed.to_ascii_lowercase();
    let qpath = if lower.ends_with(".bsp") || lower.ends_with(".map") {
        &trimmed[..trimmed.len().saturating_sub(4)]
    } else {
        trimmed
    };
    Some(qpath.replace('\\', "/"))
}

pub(in crate::renderer) fn load_ui_loading_texture(
    base: &Path,
    game: Option<&Path>,
    levelshot_map_name: Option<&str>,
    prefer_wide: bool,
) -> (TextureData, Option<[u32; 2]>) {
    if let Some(map_name) = levelshot_map_name {
        // Use the renderer's normal VFS/image resolver. Passing an extensionless
        // qpath intentionally gives levelshots the same format fallback rules as
        // every other generic texture load instead of maintaining a private list.
        let path = format!("levelshots/{map_name}");
        let wide = format!("levelshots_16_9/{map_name}");
        let data = (if prefer_wide {
            try_load_texture_asset(base, game, &wide, true, false, true, "UI widescreen levelshot")
        } else {None}).or_else(||try_load_texture_asset(base, game, &path, true, false, true, "UI levelshot"));
        if let Some(data) = data {
            let size = [data.width, data.height];
            return (data, Some(size));
        }
        eprintln!(
            "UI levelshot: no image resolved for {path}; falling back to menu/art/unknownmap_mp"
        );
        return load_ui_unknown_map_texture(base, game, prefer_wide);
    }

    let splash = (if prefer_wide {
        try_load_texture_asset(base, game, "menu/splash_16_9", true, false, true, "UI widescreen splash")
    } else { None }).or_else(||try_load_texture_asset(base, game, "menu/splash", true, false, true, "UI splash"));
    if let Some(data) = splash {
        let size = [data.width, data.height];
        return (data, Some(size));
    }

    (
        TextureData {
            label: "JKA missing splash".into(),
            source: None,
            width: 1,
            height: 1,
            rgba: (vec![0, 0, 0, 255]).into(),
            rgba16f: None,
            mip_level_count: 1,
            clamp: true,
            srgb: true,
        },
        None,
    )
}

pub(in crate::renderer) fn load_ui_unknown_map_texture(
    base: &Path,
    game: Option<&Path>,
    prefer_wide: bool,
) -> (TextureData, Option<[u32; 2]>) {
    // Prefer the author-provided widescreen variant, as TaystJK does.
    if prefer_wide {
        if let Some(data) = try_load_texture_asset(base,game,"menu/art/unknownmap_mp_16_9",
            true,false,true,"UI widescreen unknown map") {
            let size=[data.width,data.height];
            return (data,Some(size));
        }
    }
    if let Some(data) = try_load_texture_asset(
        base,
        game,
        "menu/art/unknownmap_mp",
        true,
        false,
        true,
        "UI unknown map",
    ) {
        let size = [data.width, data.height];
        return (data, Some(size));
    }

    // Last-resort compatibility for incomplete/custom data installs.
    if let Some(data) = try_load_texture_asset(
        base,
        game,
        "menu/splash",
        true,
        false,
        true,
        "UI splash fallback",
    ) {
        let size = [data.width, data.height];
        return (data, Some(size));
    }

    (
        TextureData {
            label: "JKA missing unknown-map fallback".into(),
            source: None,
            width: 1,
            height: 1,
            rgba: (vec![0, 0, 0, 255]).into(),
            rgba16f: None,
            mip_level_count: 1,
            clamp: true,
            srgb: true,
        },
        None,
    )
}

pub(in crate::renderer) fn empty_ui_font_texture(label: &str) -> TextureData {
    TextureData {
        label: label.into(),
        source: None,
        width: 1,
        height: 1,
        rgba: (vec![255, 255, 255, 0]).into(),
        rgba16f: None,
        mip_level_count: 1,
        clamp: true,
        srgb: false,
    }
}

pub(in crate::renderer) fn load_ui_proportional_font(
    base: &Path,
    game: Option<&Path>,
    name: &str,
) -> (TextureData, Option<ui::ProportionalFont>) {
    let mut assets = match AssetSearchPath::open_game(base, game) {
        Ok(assets) => assets,
        Err(error) => {
            eprintln!("UI proportional font asset search: {error}");
            return (empty_ui_font_texture("JKA missing proportional font"), None);
        }
    };

    let texture_path = format!("fonts/{name}");
    let texture = try_load_ui_font_asset(&mut assets, &texture_path)
        .unwrap_or_else(|| empty_ui_font_texture("JKA missing proportional font"));

    let metrics_path = format!("fonts/{name}.fontdat");
    let metrics = match assets.read(&metrics_path, 64 * 1024) {
        Ok(Some(asset)) => match ui::parse_fontdat(&asset.bytes) {
            Ok(font) => {
                if crate::logging::renderer_verbose_enabled(2) {
                    crate::logging::write_line_with_path(
                        crate::logging::Level::Info,
                        format_args!(
                            "UI proportional font: {} point / {} px, {} from {}",
                            font.point_size,
                            font.height,
                            metrics_path,
                            asset.source.display()
                        ),
                        asset.source.clone(),
                    );
                }
                Some(font)
            }
            Err(error) => {
                eprintln!("UI proportional font {metrics_path}: {error}");
                None
            }
        },
        Ok(None) => {
            eprintln!("UI proportional font: {metrics_path} not found");
            None
        }
        Err(error) => {
            eprintln!("UI proportional font {metrics_path}: {error}");
            None
        }
    };

    (texture, metrics)
}

/// Clean UI font for the scoreboard and kill feed: a packaged
/// `fonts/ui_clean.ttf`/`.otf` when present, else Segoe UI Semibold, Segoe UI
/// or Arial from the system. `None` keeps those surfaces on the JKA font.
pub(in crate::renderer) fn load_ui_clean_font(
    base: &Path,
    game: Option<&Path>,
) -> (TextureData, Option<ui::ProportionalFont>) {
    const MAX_FONT_BYTES: usize = 64 * 1024 * 1024;
    let mut assets = AssetSearchPath::open_game(base, game).ok();
    let packaged = ["fonts/ui_clean.ttf", "fonts/ui_clean.otf"].into_iter().filter_map(|path| {
        let bytes = assets.as_mut()?.read(path, MAX_FONT_BYTES).ok().flatten()?.bytes;
        Some((path.to_owned(), bytes))
    });
    let system = ui::system_font_dirs().into_iter().flat_map(|dir| {
        ["seguisb.ttf", "segoeui.ttf", "arial.ttf"].map(|name| dir.join(name))
    });
    let candidates = packaged.collect::<Vec<_>>().into_iter().chain(
        system.filter_map(|path| Some((path.display().to_string(), std::fs::read(&path).ok()?))),
    );
    for (label, bytes) in candidates {
        match ui::rasterize_ttf_font(&bytes) {
            Ok(font) => {
                let texture = TextureData {
                    label: format!("UI clean font {label}"),
                    source: None,
                    width: font.width,
                    height: font.height,
                    rgba: font.rgba.into(),
                    rgba16f: None,
                    mip_level_count: font.mip_level_count,
                    clamp: true,
                    srgb: false,
                };
                return (texture, Some(font.font));
            }
            Err(error) => eprintln!("UI clean font {label}: {error}"),
        }
    }
    (empty_ui_font_texture("JKA missing clean UI font"), None)
}

/// Studio overlay font for `choice`: `ui::STUDIO_FONT_CLASSIC` is JKA's
/// ergoec; a path is a font file on disk or in the game filesystem; empty
/// picks a packaged `fonts/studio_overlay.ttf`, else a system UI font. Any
/// failure falls back down that chain to ergoec.
pub(in crate::renderer) fn load_studio_overlay_font(
    base: &Path,
    game: Option<&Path>,
    choice: &str,
) -> (TextureData, Option<ui::ProportionalFont>) {
    const MAX_FONT_BYTES: usize = 64 * 1024 * 1024;
    if choice == ui::STUDIO_FONT_CLASSIC {
        return load_ui_proportional_font(base, game, "ergoec");
    }
    let mut assets = AssetSearchPath::open_game(base, game).ok();
    let mut read_game = |path: &str| {
        assets.as_mut()?.read(path, MAX_FONT_BYTES).ok().flatten().map(|a| a.bytes)
    };
    // (label, read from the game filesystem rather than disk)
    let mut sources: Vec<(String, bool)> = Vec::new();
    if !choice.is_empty() {
        sources.push((choice.to_owned(), !Path::new(choice).is_absolute()));
    }
    sources.extend(ui::STUDIO_PACKAGED_FONTS.iter().map(|p| ((*p).to_owned(), true)));
    sources.extend(ui::studio_default_system_fonts().into_iter().map(|p| (p.display().to_string(), false)));
    for (label, in_game) in sources {
        let bytes = if in_game { read_game(&label) } else { std::fs::read(&label).ok() };
        let Some(bytes) = bytes else { continue };
        match ui::rasterize_ttf_font(&bytes) {
            Ok(font) => {
                let texture = TextureData {
                    label: format!("Studio overlay font {label}"),
                    source: None,
                    width: font.width,
                    height: font.height,
                    rgba: font.rgba.into(),
                    rgba16f: None,
                    mip_level_count: font.mip_level_count,
                    clamp: true,
                    srgb: false,
                };
                return (texture, Some(font.font));
            }
            Err(error) => eprintln!("Studio overlay font {label}: {error}"),
        }
    }
    load_ui_proportional_font(base, game, "ergoec")
}

/// A single authored shader stage inserted at its original painter-order
/// position inside the native UI stream. No extra render pass is required.
#[derive(Clone)]
pub(in crate::renderer) struct UiIconShaderBatch {
    pub(in crate::renderer) layer: u8, // 0 dynamic, 1 transient, 2 retained
    pub(in crate::renderer) original_start: u32,
    pub(in crate::renderer) original_end: u32,
    pub(in crate::renderer) stage: ScreenFxBatch,
    /// Script-owned shader key and dynamic uniform byte offset, or stock FX.
    pub(in crate::renderer) user: Option<(String, u32)>,
}

#[derive(Clone)]
pub(in crate::renderer) struct ScreenFxBatch {
    pub(in crate::renderer) range: std::ops::Range<u32>,
    pub(in crate::renderer) blend: FxBlend,
    pub(in crate::renderer) texture_key: Option<String>,
}

#[cfg(test)]
mod extension_image_atlas_tests {
    #[test]
    fn native_jk2_can_grow_atlas_without_changing_the_extension_limit() {
        assert_eq!(crate::extensions::MAX_HUD_IMAGES, 32);
        let small = super::blank_ui_extension_atlas();
        let jk2 = super::blank_ui_extension_atlas_for_count(76);
        assert_eq!(small.width, jk2.width);
        assert!(jk2.height > small.height);
        assert!(jk2.width as usize * jk2.height as usize / (256 * 256) >= 76);
    }

    use super::*;

    #[test]
    fn blank_atlas_has_room_for_every_slot() {
        let atlas = blank_ui_extension_atlas();
        let cells = (atlas.width / ui::EXT_IMAGE_CELL) * (atlas.height / ui::EXT_IMAGE_CELL);
        assert!(cells as usize >= crate::extensions::MAX_HUD_IMAGES);
        assert!(atlas.rgba.iter().all(|&b| b == 0));
    }

    #[test]
    #[ignore = "requires JKA_TEST_BASE with stock assets"]
    fn stock_hud_art_decodes_into_cells() {
        let base = std::path::PathBuf::from(std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE"));
        let names: Vec<String> = [
            "gfx/2d/numbers/t_one", "gfx/hud/armor_tic_1", "gfx/hud/hudleft", "gfx/hud/health_tic_1",
            "gfx/hud/nonexistent_image",
        ].iter().map(|n| (*n).to_owned()).collect();
        let cells = load_ui_extension_images(&base, None, &names);
        let opaque = |cell: &Vec<u8>| cell.chunks_exact(4).filter(|p| p[3] > 16).count();
        for cell in &cells[..4] {
            let cell = cell.as_ref().expect("stock image missing");
            assert_eq!(cell.len() as u32, ui::EXT_IMAGE_CELL * ui::EXT_IMAGE_CELL * 4);
            assert!(opaque(cell) > 500);
        }
        assert!(cells[4].is_none(), "a missing image must stay transparent");
    }
}
