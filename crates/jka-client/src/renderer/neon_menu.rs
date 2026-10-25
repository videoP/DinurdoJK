//! The main page: a rainy café window at night, drawn entirely by the
//! renderer. Pass order, all in one encoder:
//!
//! 1. neon tubes + lightbox  -> `signs` (HDR emission)
//! 2. blur chain on signs    -> halo in the wet air
//! 3. street                 -> graded live scene + signs + halo
//! 4. Toadstorm RainyGlass   -> drops on the outside of the pane (unchanged port)
//! 5. blur chain on street   -> what condensation diffuses
//! 6. window                 -> condensation, hand wipes, liquid-glass beads,
//!    wet sill, film; into the presentation target.
//!
//! Before step 1, CRT text + crt-lottes (unchanged port) render the picture
//! that step 3 maps onto the scene's wall screen through its perspective.
//!
//! The app lays the page out with Taffy (`crate::neon_menu::layout`) and sends
//! a `MenuFrame` on change; all animation happens here (`anim.rs`).
mod anim;
mod pipelines;
mod text;

use crate::neon_menu::{font, Group, MenuFrame, ITEMS, ITEM_COUNT, TITLE};
use crate::pipeline_jobs::{hash_config as pipeline_hash, PipelineJobKey};
use crate::renderer::{ui, Renderer};
use crate::ui::ProportionalFont;
use crate::weather::RainGlassResources;
use anim::{Anim, Geometry};
use bytemuck::{Pod, Zeroable};
use jka_assets::pk3::AssetSearchPath;
use pipelines::Pipelines;
use std::time::Instant;
use wgpu::util::DeviceExt;

pub(super) const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub(super) const CRT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const CRT_SOURCE: [u32; 2] = [320, 240];
const BLUR_LEVELS: u32 = 6;
const TITLE_LINE: usize = ITEM_COUNT;
const SABER_BLADE: usize = ITEM_COUNT + 1;
const SABER_HILT: usize = ITEM_COUNT + 2;
/// Sign state entries: the items, the title, the saber blade and hilt.
const LIGHTS: usize = ITEM_COUNT + 3;
/// Power-on front far right of any sign: the whole tube is lit.
const FULLY_LIT: f32 = 1.0e6;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ViewUniform {
    size: [f32; 4],
    time: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default, Pod, Zeroable)]
struct ItemLight {
    color: [f32; 4],
    state: [f32; 4],
    fault: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GlyphInstance {
    rect: [f32; 4],
    range: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct OutsideUniform {
    control: [f32; 4],
    tone: [f32; 4],
    cache_uv: [f32; 4],
    tv_inverse: [[f32; 4]; 3],
    tv: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct WindowUniform {
    size: [f32; 4],
    fog: [f32; 4],
    sill: [f32; 4],
    film: [f32; 4],
    counts: [f32; 4],
    wipes: [[f32; 4]; 18],
    drips: [[f32; 4]; 32],
}

/// Same layout as crt_lottes.wgsl's `CrtUniform`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CrtUniform {
    output_input_size: [f32; 4],
    filter_params: [f32; 4],
    mask_corner: [f32; 4],
}

/// Gas colours, linear. Wakui's palette: rose-red, cyan, sodium amber, and a
/// cold argon white.
fn gas(group: Group) -> [f32; 3] {
    match group {
        Group::Hero => [1.0, 0.09, 0.28],
        Group::Primary => [0.04, 0.80, 1.0],
        Group::Utility => [1.0, 0.40, 0.07],
        Group::Exit => [0.55, 0.66, 1.0],
    }
}
const TITLE_GAS: [f32; 3] = [0.80, 0.88, 1.0];

struct Fonts {
    aurebesh: ProportionalFont,
    latin: ProportionalFont,
    _textures: [wgpu::Texture; 2],
    aurebesh_view: wgpu::TextureView,
    latin_view: wgpu::TextureView,
}

struct Chain {
    _textures: [wgpu::Texture; 2],
    levels: u32,
    down_all: wgpu::TextureView,
    down_mips: Vec<wgpu::TextureView>,
    up_mips: Vec<wgpu::TextureView>,
    down_groups: Vec<wgpu::BindGroup>,
    up_groups: Vec<wgpu::BindGroup>,
}

impl Chain {
    fn new(
        device: &wgpu::Device,
        p: &Pipelines,
        sampler: &wgpu::Sampler,
        input: &wgpu::TextureView,
        width: u32,
        height: u32,
        label: &str,
    ) -> Self {
        let base = [(width / 2).max(1), (height / 2).max(1)];
        let fit = 32 - base[0].min(base[1]).leading_zeros();
        let levels = BLUR_LEVELS.min(fit).max(2);
        let texture = |name: &str, mips: u32| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(name),
                size: wgpu::Extent3d { width: base[0], height: base[1], depth_or_array_layers: 1 },
                mip_level_count: mips,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: HDR,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let down = texture(&format!("{label} down"), levels);
        let up = texture(&format!("{label} up"), levels - 1);
        let mip = |t: &wgpu::Texture, level: u32| {
            t.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: level,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let down_mips: Vec<_> = (0..levels).map(|level| mip(&down, level)).collect();
        let up_mips: Vec<_> = (0..levels - 1).map(|level| mip(&up, level)).collect();
        let group = |source: &wgpu::TextureView, current: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &p.blur_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(source) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(current) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
                ],
            })
        };
        let down_groups = (0..levels as usize)
            .map(|i| {
                let source = if i == 0 { input } else { &down_mips[i - 1] };
                group(source, source)
            })
            .collect();
        let up_groups = (0..levels as usize - 1)
            .map(|i| {
                let lower = if i == levels as usize - 2 { &down_mips[i + 1] } else { &up_mips[i + 1] };
                group(lower, &down_mips[i])
            })
            .collect();
        Self {
            down_all: down.create_view(&wgpu::TextureViewDescriptor::default()),
            _textures: [down, up],
            levels,
            down_mips,
            up_mips,
            down_groups,
            up_groups,
        }
    }

    fn result(&self) -> &wgpu::TextureView {
        &self.up_mips[0]
    }

    fn encode(&self, encoder: &mut wgpu::CommandEncoder, p: &Pipelines) {
        for (target, group) in self.down_mips.iter().zip(&self.down_groups) {
            fullscreen(encoder, "JKA neon menu blur down", &p.blur_down, group, target, None);
        }
        for i in (0..self.up_groups.len()).rev() {
            fullscreen(encoder, "JKA neon menu blur up", &p.blur_up, &self.up_groups[i], &self.up_mips[i], None);
        }
    }
}

fn fullscreen(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    pipeline: &wgpu::RenderPipeline,
    group: &wgpu::BindGroup,
    target: &wgpu::TextureView,
    clear: Option<wgpu::Color>,
) {
    let mut pass = begin(encoder, label, target, clear.unwrap_or(wgpu::Color::BLACK));
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, group, &[]);
    pass.draw(0..3, 0..1);
}

fn begin<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    label: &str,
    target: &wgpu::TextureView,
    clear: wgpu::Color,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

fn target_texture(device: &wgpu::Device, label: &str, size: [u32; 2], format: wgpu::TextureFormat) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: size[0].max(1), height: size[1].max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// Growable buffer; returns true when it was reallocated.
fn upload(device: &wgpu::Device, queue: &wgpu::Queue, slot: &mut Option<wgpu::Buffer>, label: &str, usage: wgpu::BufferUsages, bytes: &[u8]) -> bool {
    let needed = (bytes.len() as u64).max(16).next_power_of_two();
    let mut reallocated = false;
    if slot.as_ref().is_none_or(|buffer| buffer.size() < needed) {
        *slot = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: needed,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        reallocated = true;
    }
    if !bytes.is_empty() {
        queue.write_buffer(slot.as_ref().expect("allocated above"), 0, bytes);
    }
    reallocated
}

fn load_font(device: &wgpu::Device, queue: &wgpu::Queue, assets: &mut AssetSearchPath, name: &str) -> Option<(ProportionalFont, wgpu::Texture, wgpu::TextureView)> {
    let metrics = assets.read(&format!("fonts/{name}.fontdat"), 64 * 1024).ok()??;
    let font = crate::ui::parse_fontdat(&metrics.bytes).ok()?;
    let (bytes, format) = [("png", image::ImageFormat::Png), ("tga", image::ImageFormat::Tga)]
        .into_iter()
        .find_map(|(ext, format)| {
            assets.read(&format!("fonts/{name}.{ext}"), 16 << 20).ok().flatten().map(|asset| (asset.bytes, format))
        })?;
    let image = image::load_from_memory_with_format(&bytes, format).ok()?.to_rgba8();
    let texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("JKA neon menu font atlas"),
            size: wgpu::Extent3d { width: image.width(), height: image.height(), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        image.as_raw(),
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Some((font, texture, view))
}

/// Decoded scene cache, kept on the GPU independently of live BSP loading.
struct CachedScene {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: [u32; 2],
}

/// Size-dependent targets and the bind groups that reference them.
struct Targets {
    width: u32,
    height: u32,
    _signs: wgpu::Texture,
    signs: wgpu::TextureView,
    _street: wgpu::Texture,
    street: wgpu::TextureView,
    halo: Chain,
    fog: Chain,
    outside_group: wgpu::BindGroup,
    window_group: wgpu::BindGroup,
    monitor: [u32; 2],
    _crt_out: wgpu::Texture,
    crt_out: wgpu::TextureView,
}

/// Size-independent resources, created with the pipelines.
struct Shared {
    sampler: wgpu::Sampler,
    signs_view: wgpu::Buffer,
    text_hdr_view: wgpu::Buffer,
    text_crt_view: wgpu::Buffer,
    outside: wgpu::Buffer,
    window: wgpu::Buffer,
    glass: wgpu::Buffer,
    crt_uniform: wgpu::Buffer,
    _crt_source: wgpu::Texture,
    crt_source: wgpu::TextureView,
    crt_group: wgpu::BindGroup,
}

#[derive(Default)]
pub(in crate::renderer) struct NeonMenu {
    frame: Option<MenuFrame>,
    cached_scene: Option<CachedScene>,
    pipelines: Option<Pipelines>,
    pipelines_format: Option<wgpu::TextureFormat>,
    shared: Option<Shared>,
    fonts: Option<Fonts>,
    fonts_tried: bool,
    text_groups: Option<(wgpu::BindGroup, wgpu::BindGroup)>,
    rain: Option<RainGlassResources>,
    rain_tried: bool,
    targets: Option<Targets>,
    anim: Anim,
    last_tick: Option<Instant>,
    // Geometry for the current layout and size.
    built_for: Option<(crate::neon_menu::layout::MenuLayout, u32, u32, u32)>,
    prims: Option<wgpu::Buffer>,
    instances: Option<wgpu::Buffer>,
    instance_count: u32,
    items: Option<wgpu::Buffer>,
    signs_group: Option<wgpu::BindGroup>,
    text_hdr: Option<wgpu::Buffer>,
    text_hdr_count: u32,
    text_crt: Option<wgpu::Buffer>,
    text_crt_count: u32,
    geometry: Option<Geometry>,
    scene_gamma: bool,
}

fn gamma_encoded(format: wgpu::TextureFormat) -> bool {
    matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgb10a2Unorm
    )
}

impl NeonMenu {
    fn set_cached_scene(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, image: Option<egui::ColorImage>) {
        self.cached_scene = image.and_then(|image| {
            let size = [u32::try_from(image.size[0]).ok()?, u32::try_from(image.size[1]).ok()?];
            if size[0] == 0 || size[1] == 0 || image.pixels.len() != size[0] as usize * size[1] as usize {
                return None;
            }
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("JKA cached frontend scene (neon background)"),
                size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let raw: Vec<u8> = image.pixels.iter().flat_map(|pixel| pixel.to_array()).collect();
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture, mip_level: 0,
                    origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All,
                },
                &raw,
                wgpu::TexelCopyBufferLayout {
                    offset: 0, bytes_per_row: Some(size[0] * 4), rows_per_image: Some(size[1]),
                },
                texture.size(),
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            Some(CachedScene { _texture: texture, view, size })
        });
        // New texture means new bind group, not another pipeline compilation.
        self.targets = None;
    }

    fn ready(&self) -> bool {
        self.pipelines.is_some() && self.shared.is_some() && self.targets.is_some() && self.signs_group.is_some()
            && self.geometry.is_some() && self.text_groups.is_some()
    }

    fn prepare(&mut self, r: &mut Renderer) {
        let Some(frame) = self.frame.clone() else { return };
        let now = Instant::now();
        let dt = self.last_tick.map_or(0.0, |last| now.duration_since(last).as_secs_f32());
        self.last_tick = Some(now);
        let width = r.config.width.max(1);
        let height = r.config.height.max(1);
        let format = r.config.format;
        self.scene_gamma = gamma_encoded(format);

        // Pipelines compile on a worker; until then the page shows the plain
        // scene (the cached menu still covers that moment anyway).
        if self.pipelines_format != Some(format) {
            self.pipelines = None;
            self.shared = None;
            self.targets = None;
            self.text_groups = None;
            self.signs_group = None;
            self.pipelines_format = Some(format);
        }
        if self.pipelines.is_none() {
            let key = PipelineJobKey::new("neon-menu", 0, pipeline_hash(&format));
            if let Some(pipelines) = r.pipeline_jobs.take_ready::<Pipelines>(key) {
                self.pipelines = Some(pipelines);
            } else {
                let device = r.device.clone();
                r.pipeline_jobs.request(key, "neon main menu", move || pipelines::create(&device, format));
                return;
            }
        }
        let p = self.pipelines.as_ref().expect("checked above");
        let device = r.device.clone();
        let queue = r.queue.clone();

        if self.shared.is_none() {
            self.shared = Some(create_shared(&device, &queue, p));
        }
        if !self.fonts_tried {
            self.fonts_tried = true;
            if let Some(assets) = r.weather_assets.as_mut() {
                let aurebesh = load_font(&device, &queue, assets, "aurabesh");
                let latin = load_font(&device, &queue, assets, "ergoec");
                match (aurebesh, latin) {
                    (Some(a), Some(l)) => {
                        self.fonts = Some(Fonts {
                            aurebesh: a.0,
                            latin: l.0,
                            _textures: [a.1, l.1],
                            aurebesh_view: a.2,
                            latin_view: l.2,
                        });
                    }
                    _ => eprintln!("[NEON MENU] fonts/aurabesh or fonts/ergoec missing; signs draw without lettering"),
                }
            }
        }
        let shared = self.shared.as_ref().expect("created above");
        if self.text_groups.is_none() {
            let fallback;
            let (aurebesh, latin) = match &self.fonts {
                Some(fonts) => (&fonts.aurebesh_view, &fonts.latin_view),
                None => {
                    let (texture, view) = target_texture(&device, "JKA neon menu empty atlas", [1, 1], wgpu::TextureFormat::Rgba8Unorm);
                    fallback = (texture, view);
                    (&fallback.1, &fallback.1)
                }
            };
            let group = |view: &wgpu::Buffer| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("JKA neon menu text"),
                    layout: &p.text_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: view.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(aurebesh) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(latin) },
                        wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&shared.sampler) },
                    ],
                })
            };
            self.text_groups = Some((group(&shared.text_hdr_view), group(&shared.text_crt_view)));
        }

        // The Toadstorm lens pass, our own instance at HDR precision.
        if self.rain.is_none() && !self.rain_tried {
            self.rain_tried = true;
            if let Some(assets) = r.weather_assets.as_mut() {
                match RainGlassResources::new(&device, &queue, width, height, HDR, assets) {
                    Ok(mut rain) => {
                        // Never share the weather lens's job slot: the two
                        // formats would cancel each other's compile forever.
                        rain.set_job_slot(1);
                        self.rain = Some(rain);
                    }
                    Err(error) => eprintln!("[NEON MENU] RainyGlass unavailable: {error}"),
                }
            }
        }
        if let Some(rain) = self.rain.as_mut() {
            rain.prepare(&mut r.pipeline_jobs, &device, &queue, width, height, HDR, self.anim.time, 1.0, 0.0, [0.0; 4]);
        }

        // Geometry from the Taffy layout, in pixels.
        let layout = frame.layout;
        let w = width as f32;
        let h = height as f32;
        // The scene's wall screen that shows the CRT, and its bounds in pixels.
        let tv = crate::neon_menu::tv_corners(w / h);
        let monitor = [
            tv.iter().map(|c| c[0]).fold(1.0, f32::min) * w, tv.iter().map(|c| c[1]).fold(1.0, f32::min) * h,
            tv.iter().map(|c| c[0]).fold(0.0, f32::max) * w, tv.iter().map(|c| c[1]).fold(0.0, f32::max) * h,
        ];
        let monitor_size = [
            ((monitor[2] - monitor[0]).round() as u32).clamp(1, 2048),
            ((monitor[3] - monitor[1]).round() as u32).clamp(1, 2048),
        ];

        let Some(backdrop) = r.menu_backdrop.as_ref().filter(|b| b.width == width && b.height == height) else {
            return;
        };
        let rebuild_targets = self.targets.as_ref().is_none_or(|t| t.width != width || t.height != height || t.monitor != monitor_size);
        if rebuild_targets {
            self.targets = Some(create_targets(
                &device, p, shared, &backdrop.view,
                self.cached_scene.as_ref().map(|cache| &cache.view),
                self.rain.as_ref().map(RainGlassResources::source_view),
                width, height, monitor_size,
            ));
        }
        let targets = self.targets.as_ref().expect("built above");

        let letterforms = font::generation();
        if self.built_for != Some((layout, width, height, letterforms)) {
            let (prims, instances) = build_glyphs(&layout, w, h);
            let prims_moved = upload(&device, &queue, &mut self.prims, "JKA neon menu tube centre lines",
                wgpu::BufferUsages::STORAGE, bytemuck::cast_slice(&prims));
            upload(&device, &queue, &mut self.instances, "JKA neon menu glyphs",
                wgpu::BufferUsages::VERTEX, bytemuck::cast_slice(&instances));
            self.instance_count = instances.len() as u32;
            let items_moved = upload(&device, &queue, &mut self.items, "JKA neon menu sign state",
                wgpu::BufferUsages::STORAGE, bytemuck::cast_slice(&[ItemLight::default(); LIGHTS]));
            if prims_moved || items_moved || self.signs_group.is_none() {
                self.signs_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("JKA neon menu signs"),
                    layout: &p.signs_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: shared.signs_view.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: self.prims.as_ref().expect("uploaded").as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 2, resource: self.items.as_ref().expect("uploaded").as_entire_binding() },
                    ],
                }));
            }
            self.built_for = Some((layout, width, height, letterforms));
        }
        let mut geometry = Geometry {
            scale: h / 1080.0,
            wipe: [[0.0; 4]; ITEM_COUNT],
            ink: [[0.0; 2]; ITEM_COUNT],
            sill: h * 0.93,
            width: w,
            monitor,
        };
        for (index, info) in ITEMS.iter().enumerate() {
            let line = layout.items[index];
            let (x, baseline, cap) = (line.x * w, line.baseline * h, line.cap * h);
            let ink = font::measure(info.label) * cap;
            let half = if info.group == Group::Hero { 0.78 } else { 0.95 } * cap;
            let centre = baseline - cap * 0.5;
            geometry.wipe[index] = [x - 0.35 * cap, centre - half, x + ink + 0.35 * cap, centre + half];
            geometry.ink[index] = [x, x + ink];
        }
        self.anim.update(dt, &frame, &geometry);

        // Per-frame sign state.
        let anim = &self.anim;
        let mut lights = [ItemLight::default(); LIGHTS];
        for (index, info) in ITEMS.iter().enumerate() {
            let item = &anim.items[index];
            let [x0, x1] = geometry.ink[index];
            let cap = layout.items[index].cap * h;
            let c = gas(info.group);
            let front = if item.fill >= 1.0 { FULLY_LIT } else { x0 + (x1 - x0 + cap) * item.fill };
            lights[index] = ItemLight {
                color: [c[0], c[1], c[2], 0.62],
                state: [item.level, front, (cap * font::tube_ratio()).max(0.9), item.flash],
                fault: [
                    if index == anim::FAULTY_ITEM { anim::FAULTY_GLYPH as f32 } else { -1.0 },
                    anim.fault_level(),
                    index as f32,
                    if index == anim.selected { 1.0 } else { 0.0 },
                ],
            };
        }
        let title_cap = layout.title.cap * h;
        lights[TITLE_LINE] = ItemLight {
            color: [TITLE_GAS[0], TITLE_GAS[1], TITLE_GAS[2], 0.8],
            state: [anim.title_level, FULLY_LIT, (title_cap * font::tube_ratio() * 1.15).max(0.9), 0.0],
            fault: [-1.0, 1.0, 0.0, 0.0],
        };
        let saber = saber(&layout, w, h);
        let blade_len = saber.blade[1][0] - saber.blade[0][0];
        lights[SABER_BLADE] = ItemLight {
            color: [0.22, 0.52, 1.0, 0.85],
            state: [
                anim.saber_level,
                if anim.saber_fill >= 1.0 { FULLY_LIT } else { saber.blade[0][0] + (blade_len + saber.blade_tube * 4.0) * anim.saber_fill },
                saber.blade_tube,
                0.0,
            ],
            fault: [-1.0, 1.0, 0.0, 0.0],
        };
        lights[SABER_HILT] = ItemLight {
            color: [0.72, 0.76, 0.84, 0.8],
            state: [anim.title_level * 0.6, FULLY_LIT, saber.hilt_tube, 0.0],
            fault: [-1.0, 1.0, 0.0, 0.0],
        };
        queue.write_buffer(self.items.as_ref().expect("uploaded with the glyphs"), 0, bytemuck::cast_slice(&lights));
        let time = anim.time;
        queue.write_buffer(&shared.signs_view, 0, bytemuck::bytes_of(&ViewUniform { size: [w, h, 0.0, 0.0], time: [time, 0.0, 0.0, 0.0] }));
        queue.write_buffer(&shared.text_hdr_view, 0, bytemuck::bytes_of(&ViewUniform { size: [w, h, time, 0.0], time: [0.0; 4] }));
        queue.write_buffer(&shared.text_crt_view, 0, bytemuck::bytes_of(&ViewUniform {
            size: [CRT_SOURCE[0] as f32, CRT_SOURCE[1] as f32, time, 0.0],
            time: [0.0; 4],
        }));

        // Lightbox and CRT lettering.
        let mut hdr_text = Vec::new();
        let mut crt_text = Vec::new();
        if let Some(fonts) = &self.fonts {
            lightbox_text(&mut hdr_text, fonts, anim, &layout, w, h);
            monitor_text(&mut crt_text, fonts, anim);
        }
        upload(&device, &queue, &mut self.text_hdr, "JKA neon menu lightbox quads", wgpu::BufferUsages::VERTEX, bytemuck::cast_slice(&hdr_text));
        upload(&device, &queue, &mut self.text_crt, "JKA neon menu CRT quads", wgpu::BufferUsages::VERTEX, bytemuck::cast_slice(&crt_text));
        self.text_hdr_count = hdr_text.len() as u32;
        self.text_crt_count = crt_text.len() as u32;

        let reveal = anim.revealed_at.map_or(0.0, |at| ((time - at) / 0.8).clamp(0.0, 1.0));
        let cache_mix = if self.cached_scene.is_some() {
            1.0 - frame.live_scene_blend.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let cache_uv = self.cached_scene.as_ref().map_or([1.0, 1.0, 0.0, 0.0], |cached| {
            let cover = (w / cached.size[0] as f32).max(h / cached.size[1] as f32);
            [w / (cached.size[0] as f32 * cover), h / (cached.size[1] as f32 * cover), 0.0, 0.0]
        });
        queue.write_buffer(&shared.outside, 0, bytemuck::bytes_of(&OutsideUniform {
            control: [if self.scene_gamma { 1.0 } else { 0.0 }, time, 0.85, 1.0],
            tone: [0.62, targets.halo.levels as f32, cache_mix, 0.0],
            cache_uv,
            tv_inverse: screen_to_quad(&tv).map(|c| [c[0], c[1], c[2], 0.0]),
            tv: [1.3, 1.0, 0.0, 0.0],
        }));
        let mut window = WindowUniform::zeroed();
        window.size = [w, h, time, if gamma_encoded(format) { 1.0 } else { 0.0 }];
        window.fog = [0.56, 2.3, 0.8, 0.55];
        window.sill = [geometry.sill, 0.9, 1.0, reveal];
        window.film = [0.05, 0.035, 0.32, targets.fog.levels as f32];
        // Wiping an item also clears the rain drops, when the lens pass ran
        // (its source is the dry street). The TV patch keeps its drops.
        let rain_active = self.rain.as_ref().is_some_and(RainGlassResources::active);
        window.counts = [
            ITEM_COUNT as f32 + 1.0,
            anim.drips.len().min(16) as f32,
            if rain_active { ITEM_COUNT as f32 } else { 0.0 },
            0.0,
        ];
        // Someone wiped a patch to watch the TV; it never fogs over.
        let pad = [(monitor[2] - monitor[0]) * 0.06, (monitor[3] - monitor[1]) * 0.06];
        window.wipes[ITEM_COUNT * 2] = [monitor[0] - pad[0], monitor[1] - pad[1], monitor[2] + pad[0], monitor[3] + pad[1]];
        window.wipes[ITEM_COUNT * 2 + 1] = [1.0, 1.0, 0.0, 0.37];
        for (index, item) in anim.items.iter().enumerate() {
            let wipe = item.wipe;
            // The hand accelerates into the stroke and slows at the end.
            let t = wipe.progress.clamp(0.0, 1.0);
            let eased = t * t * (3.0 - 2.0 * t);
            window.wipes[index * 2] = geometry.wipe[index];
            window.wipes[index * 2 + 1] = [if wipe.progress > 0.0 { eased.max(0.001) } else { 0.0 }, wipe.clear.max(0.0), wipe.tilt, wipe.seed];
        }
        for (index, drip) in anim.drips.iter().take(16).enumerate() {
            window.drips[index * 2] = [drip.x, drip.y, drip.r, drip.alpha];
            window.drips[index * 2 + 1] = [drip.top, drip.trail, drip.stretch, drip.seed];
        }
        queue.write_buffer(&shared.window, 0, bytemuck::bytes_of(&window));
        queue.write_buffer(&shared.crt_uniform, 0, bytemuck::bytes_of(&CrtUniform {
            output_input_size: [monitor_size[0] as f32, monitor_size[1] as f32, CRT_SOURCE[0] as f32, CRT_SOURCE[1] as f32],
            filter_params: [0.0; 4],
            mask_corner: [0.0; 4],
        }));
        self.geometry = Some(geometry);
    }

    fn encode(&self, encoder: &mut wgpu::CommandEncoder, output: &wgpu::TextureView, plain_scene: bool) -> bool {
        let (Some(p), Some(shared), Some(targets), Some(signs_group), Some(text_groups)) =
            (&self.pipelines, &self.shared, &self.targets, &self.signs_group, &self.text_groups)
        else {
            return false;
        };
        if plain_scene {
            fullscreen(encoder, "JKA neon menu plain scene", &p.passthrough, &targets.window_group, output, None);
            return true;
        }
        {
            let mut pass = begin(encoder, "JKA neon menu signs", &targets.signs, wgpu::Color::BLACK);
            if let Some(instances) = self.instances.as_ref().filter(|_| self.instance_count > 0) {
                pass.set_pipeline(&p.signs);
                pass.set_bind_group(0, signs_group, &[]);
                pass.set_vertex_buffer(0, instances.slice(..));
                pass.draw(0..6, 0..self.instance_count);
            }
            if let Some(quads) = self.text_hdr.as_ref().filter(|_| self.text_hdr_count > 0) {
                pass.set_pipeline(&p.text_hdr);
                pass.set_bind_group(0, &text_groups.0, &[]);
                pass.set_vertex_buffer(0, quads.slice(..));
                pass.draw(0..self.text_hdr_count, 0..1);
            }
        }
        // The CRT picture first: the street pass puts it on the wall screen.
        {
            let mut pass = begin(encoder, "JKA neon menu CRT source", &shared.crt_source,
                wgpu::Color { r: 0.0035, g: 0.0045, b: 0.008, a: 1.0 });
            if let Some(quads) = self.text_crt.as_ref().filter(|_| self.text_crt_count > 0) {
                pass.set_pipeline(&p.text_crt);
                pass.set_bind_group(0, &text_groups.1, &[]);
                pass.set_vertex_buffer(0, quads.slice(..));
                pass.draw(0..self.text_crt_count, 0..1);
            }
        }
        fullscreen(encoder, "JKA neon menu crt-lottes", &p.crt, &shared.crt_group, &targets.crt_out, None);
        targets.halo.encode(encoder, p);
        let rain = self.rain.as_ref().filter(|rain| rain.active());
        let street_target = rain.map_or(&targets.street, |rain| rain.source_view());
        fullscreen(encoder, "JKA neon menu street", &p.outside, &targets.outside_group, street_target, None);
        if let Some(rain) = rain {
            rain.render(encoder, &targets.street);
        }
        targets.fog.encode(encoder, p);
        fullscreen(encoder, "JKA neon menu window", &p.window, &targets.window_group, output, None);
        true
    }
}

fn create_shared(device: &wgpu::Device, queue: &wgpu::Queue, p: &Pipelines) -> Shared {
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("JKA neon menu sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    });
    let uniform = |label: &str, size: usize| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: size as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    };
    let glass = crate::glass::Settings::preset(crate::glass::Style::Liquid).uniform([0.05, 0.07, 0.09]);
    let glass = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("JKA neon menu Liquid glass"),
        contents: bytemuck::bytes_of(&glass),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let (crt_texture, crt_source) = target_texture(device, "JKA neon menu CRT source", CRT_SOURCE, CRT_FORMAT);
    let crt_uniform = uniform("JKA neon menu crt-lottes", std::mem::size_of::<CrtUniform>());
    queue.write_buffer(&crt_uniform, 0, bytemuck::bytes_of(&CrtUniform::zeroed()));
    let crt_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA neon menu crt-lottes"),
        layout: &p.crt_layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&crt_source) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
            wgpu::BindGroupEntry { binding: 2, resource: crt_uniform.as_entire_binding() },
        ],
    });
    Shared {
        signs_view: uniform("JKA neon menu signs view", std::mem::size_of::<ViewUniform>()),
        text_hdr_view: uniform("JKA neon menu lightbox view", std::mem::size_of::<ViewUniform>()),
        text_crt_view: uniform("JKA neon menu CRT view", std::mem::size_of::<ViewUniform>()),
        outside: uniform("JKA neon menu street", std::mem::size_of::<OutsideUniform>()),
        window: uniform("JKA neon menu window", std::mem::size_of::<WindowUniform>()),
        glass,
        crt_uniform,
        _crt_source: crt_texture,
        crt_source,
        crt_group,
        sampler,
    }
}

fn create_targets(
    device: &wgpu::Device,
    p: &Pipelines,
    shared: &Shared,
    scene: &wgpu::TextureView,
    cached_scene: Option<&wgpu::TextureView>,
    dry_street: Option<&wgpu::TextureView>,
    width: u32,
    height: u32,
    monitor: [u32; 2],
) -> Targets {
    let (signs_texture, signs) = target_texture(device, "JKA neon menu signs", [width, height], HDR);
    let (street_texture, street) = target_texture(device, "JKA neon menu street", [width, height], HDR);
    let halo = Chain::new(device, p, &shared.sampler, &signs, width, height, "JKA neon menu halo");
    let fog = Chain::new(device, p, &shared.sampler, &street, width, height, "JKA neon menu condensation");
    let (crt_texture, crt_out) = target_texture(device, "JKA neon menu CRT", monitor, CRT_FORMAT);
    fn tex(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
        wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(view) }
    }
    let outside_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA neon menu street"),
        layout: &p.outside_layout,
        entries: &[
            tex(0, scene), tex(1, &signs), tex(2, halo.result()),
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&shared.sampler) },
            wgpu::BindGroupEntry { binding: 4, resource: shared.outside.as_entire_binding() },
            tex(5, cached_scene.unwrap_or(scene)),
            tex(6, &crt_out),
        ],
    });
    let window_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA neon menu window"),
        layout: &p.window_layout,
        entries: &[
            tex(0, &street), tex(1, &fog.down_all), tex(2, fog.result()),
            wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&shared.sampler) },
            wgpu::BindGroupEntry { binding: 5, resource: shared.window.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 6, resource: shared.glass.as_entire_binding() },
            tex(7, scene),
            tex(8, dry_street.unwrap_or(&street)),
        ],
    });
    Targets {
        width,
        height,
        _signs: signs_texture,
        signs,
        _street: street_texture,
        street,
        halo,
        fog,
        outside_group,
        window_group,
        monitor,
        _crt_out: crt_texture,
        crt_out,
    }
}

/// Homography columns taking view UV to the UV of the quad `corners`
/// (TL, TR, BR, BL): the inverse of Heckbert's square-to-quad mapping.
fn screen_to_quad(corners: &[[f32; 2]; 4]) -> [[f32; 3]; 3] {
    let [p0, p1, p2, p3] = *corners;
    let (dx1, dx2, dx3) = (p1[0] - p2[0], p3[0] - p2[0], p0[0] - p1[0] + p2[0] - p3[0]);
    let (dy1, dy2, dy3) = (p1[1] - p2[1], p3[1] - p2[1], p0[1] - p1[1] + p2[1] - p3[1]);
    let det = dx1 * dy2 - dx2 * dy1;
    let g = (dx3 * dy2 - dx2 * dy3) / det;
    let h = (dx1 * dy3 - dx3 * dy1) / det;
    let forward = glam::Mat3::from_cols(
        glam::Vec3::new(p1[0] - p0[0] + g * p1[0], p1[1] - p0[1] + g * p1[1], g),
        glam::Vec3::new(p3[0] - p0[0] + h * p3[0], p3[1] - p0[1] + h * p3[1], h),
        glam::Vec3::new(p0[0], p0[1], 1.0),
    );
    let inverse = forward.inverse();
    [inverse.x_axis.to_array(), inverse.y_axis.to_array(), inverse.z_axis.to_array()]
}

/// The lightsaber under the title, all neon: a bent-tube hilt outline and one
/// blade tube. Hilt to tip spans exactly the title's ink width.
struct Saber {
    hilt: Vec<[[f32; 2]; 2]>,
    blade: [[f32; 2]; 2],
    hilt_tube: f32,
    blade_tube: f32,
}

fn saber(layout: &crate::neon_menu::layout::MenuLayout, w: f32, h: f32) -> Saber {
    let title = layout.title;
    let (x0, baseline, cap) = (title.x * w, title.baseline * h, title.cap * h);
    let x1 = x0 + font::measure(TITLE) * cap;
    let y = baseline + 0.75 * cap;
    let hilt_tube = (cap * font::tube_ratio()).max(0.9);
    let blade_tube = (cap * 0.075).max(1.2);
    let length = 1.35 * cap;
    let half = 0.17 * cap;
    let (left, emitter) = (x0 + hilt_tube, x0 + length);
    let ridge = |t: f32| [[x0 + length * t, y - half], [x0 + length * t, y + half]];
    let hilt = vec![
        [[left, y - half], [emitter, y - half]],
        [[left, y + half], [emitter, y + half]],
        [[left, y - half], [left, y + half]],
        ridge(0.36),
        ridge(0.50),
        ridge(0.64),
        // Emitter shroud, flared wider than the grip.
        [[emitter, y - half * 1.6], [emitter, y + half * 1.6]],
    ];
    let blade = [[emitter + blade_tube * 2.5, y], [x1 - blade_tube, y]];
    Saber { hilt, blade, hilt_tube, blade_tube }
}

/// Tube centre lines in pixels, two vec4 per primitive, and one instance per
/// glyph. Font space is +y up; the screen is +y down, so arcs flip direction.
fn build_glyphs(layout: &crate::neon_menu::layout::MenuLayout, w: f32, h: f32) -> (Vec<[f32; 4]>, Vec<GlyphInstance>) {
    let mut prims = Vec::new();
    let mut instances = Vec::new();
    let lines = ITEMS.iter().enumerate().map(|(i, info)| (i, info.label, layout.items[i]))
        .chain(std::iter::once((TITLE_LINE, TITLE, layout.title)));
    for (line_index, label, line) in lines {
        let (ox, baseline, cap) = (line.x * w, line.baseline * h, line.cap * h);
        let tube = (cap * font::tube_ratio() * if line_index == TITLE_LINE { 1.15 } else { 1.0 }).max(0.9);
        let margin = tube * 7.0 + 2.0;
        for (glyph_index, placed) in font::place(label).iter().enumerate() {
            let start = (prims.len() / 2) as u32;
            let to_screen = |x: f32, y: f32| [ox + (placed.x + x) * cap, baseline - y * cap];
            let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
            for prim in placed.glyph.prims.iter() {
                match *prim {
                    font::Prim::Line { a, b } => {
                        let (pa, pb) = (to_screen(a[0], a[1]), to_screen(b[0], b[1]));
                        prims.push([0.0, pa[0], pa[1], pb[0]]);
                        prims.push([pb[1], 0.0, 0.0, 0.0]);
                    }
                    font::Prim::Arc { c, r, start, sweep } => {
                        let pc = to_screen(c[0], c[1]);
                        let mut from = -start.to_radians();
                        let mut span = -sweep.to_radians();
                        if span < 0.0 {
                            from += span;
                            span = -span;
                        }
                        prims.push([1.0, pc[0], pc[1], r * cap]);
                        prims.push([from, span, 0.0, 0.0]);
                    }
                }
                let b = font::bounds(prim);
                let lo = to_screen(b[0], b[3]);
                let hi = to_screen(b[2], b[1]);
                bounds = [bounds[0].min(lo[0]), bounds[1].min(lo[1]), bounds[2].max(hi[0]), bounds[3].max(hi[1])];
            }
            instances.push(GlyphInstance {
                rect: [bounds[0] - margin, bounds[1] - margin, bounds[2] + margin, bounds[3] + margin],
                range: [start, placed.glyph.prims.len() as u32, line_index as u32, glyph_index as u32],
            });
        }
    }
    let saber = saber(layout, w, h);
    for (line_index, lines, tube) in [
        (SABER_HILT, saber.hilt.clone(), saber.hilt_tube),
        (SABER_BLADE, vec![saber.blade], saber.blade_tube),
    ] {
        let start = (prims.len() / 2) as u32;
        let margin = tube * 7.0 + 2.0;
        let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for [a, b] in &lines {
            prims.push([0.0, a[0], a[1], b[0]]);
            prims.push([b[1], 0.0, 0.0, 0.0]);
            bounds = [bounds[0].min(a[0].min(b[0])), bounds[1].min(a[1].min(b[1])),
                bounds[2].max(a[0].max(b[0])), bounds[3].max(a[1].max(b[1]))];
        }
        instances.push(GlyphInstance {
            rect: [bounds[0] - margin, bounds[1] - margin, bounds[2] + margin, bounds[3] + margin],
            range: [start, lines.len() as u32, line_index as u32, 0],
        });
    }
    (prims, instances)
}

/// The vertical backlit sign: the selected entry's name in Aurebesh.
fn lightbox_text(out: &mut Vec<text::TextVertex>, fonts: &Fonts, anim: &Anim, layout: &crate::neon_menu::layout::MenuLayout, w: f32, h: f32) {
    let r = [layout.lightbox[0] * w, layout.lightbox[1] * h, layout.lightbox[2] * w, layout.lightbox[3] * h];
    if r[2] - r[0] < 4.0 {
        return;
    }
    let power = anim.lightbox_power;
    let size = [r[2] - r[0], r[3] - r[1]];
    let face = [1.0, 0.93, 0.84];
    // Kept well under the tube brightness: its own halo would otherwise wash
    // the lettering out.
    let glow = 0.04 + 0.62 * power;
    text::quad(out, r, [0.0, 0.0, 1.0, 1.0], [face[0] * glow, face[1] * glow, face[2] * glow, 1.0], [text::PANEL, size[0], size[1], 0.0]);
    let ink = 0.02 + 0.2 * power;
    let label = ITEMS[anim.lightbox_item].label;
    text::vertical(out, &fonts.aurebesh, label, (r[0] + r[2]) * 0.5, r[1] + size[0] * 0.45, r[3] - size[0] * 0.45,
        size[0] * 0.56, [ink, ink * 0.04, ink * 0.10, 1.0]);
}

/// The CRT's 320x240 picture: channel bug, the entry name, its description
/// typed out, and an Aurebesh ticker.
fn monitor_text(out: &mut Vec<text::TextVertex>, fonts: &Fonts, anim: &Anim) {
    let power = anim.monitor_power;
    if power <= 0.0 {
        return;
    }
    let time = anim.time;
    let index = anim.selected;
    let info = &ITEMS[index];
    let c = gas(info.group);
    let k = power;
    let dim = [0.20 * k, 0.55 * k, 0.62 * k, 1.0];
    let paper = [0.72 * k, 0.88 * k, 0.92 * k, 1.0];
    let latin = &fonts.latin;
    text::line(out, latin, text::LATIN, "DNRD-TV", 12.0, 24.0, 1.0, dim);
    let channel = format!("CH {:02}", index + 1);
    let channel_w = text::width(latin, &channel, 1.0);
    text::line(out, latin, text::LATIN, &channel, 308.0 - channel_w, 24.0, 1.0, dim);
    if (time * 1.2).fract() < 0.6 {
        text::rect(out, [308.0 - channel_w - 14.0, 15.0, 308.0 - channel_w - 8.0, 21.0], [0.9 * k, 0.08 * k, 0.12 * k, 1.0]);
    }
    text::rect(out, [12.0, 31.0, 308.0, 32.0], [0.12 * k, 0.3 * k, 0.36 * k, 1.0]);
    let name_scale = if text::width(latin, info.label, 2.0) > 296.0 { 1.5 } else { 2.0 };
    text::line(out, latin, text::LATIN, info.label, 12.0, 74.0, name_scale, [c[0] * k, c[1] * k, c[2] * k, 1.0]);

    // Teletype: the description types itself out after each selection.
    let typed = ((time - anim.select_time - 0.15).max(0.0) * 60.0) as usize;
    let mut remaining = typed;
    let mut baseline = 104.0;
    let mut cursor = (12.0, baseline);
    for line in text::wrap(latin, info.blurb, 1.0, 296.0).iter().take(5) {
        let shown: String = line.chars().take(remaining).collect();
        remaining = remaining.saturating_sub(line.chars().count() + 1);
        let end = text::line(out, latin, text::LATIN, &shown, 12.0, baseline, 1.0, paper);
        cursor = (end, baseline);
        baseline += 18.0;
        if remaining == 0 {
            break;
        }
    }
    if (time * 2.0).fract() < 0.55 {
        text::rect(out, [cursor.0 + 2.0, cursor.1 - 12.0, cursor.0 + 9.0, cursor.1 + 1.0], paper);
    }

    // Aurebesh ticker along the bottom.
    let ticker = format!("{}   ", info.label).repeat(6);
    let span = text::width(&fonts.aurebesh, &format!("{}   ", info.label), 0.8).max(1.0);
    let x = 12.0 - (time * 28.0) % span;
    text::rect(out, [0.0, 206.0, 320.0, 207.0], [0.1 * k, 0.04 * k, 0.08 * k, 1.0]);
    text::line(out, &fonts.aurebesh, text::AUREBESH, &ticker, x, 228.0, 0.8, [0.85 * k, 0.16 * k, 0.45 * k, 1.0]);

    // Power-on: the picture opens from a bright line.
    if power < 1.0 {
        let half = 120.0 * power;
        text::rect(out, [0.0, 0.0, 320.0, 120.0 - half], [0.0, 0.0, 0.0, 1.0]);
        text::rect(out, [0.0, 120.0 + half, 320.0, 240.0], [0.0, 0.0, 0.0, 1.0]);
        let line = 1.0 - power;
        text::rect(out, [0.0, 119.0 - half.min(1.0), 320.0, 121.0 + half.min(1.0)], [line, line, line, 1.0]);
    }
}

impl Renderer {
    pub(in crate::renderer) fn set_neon_menu_cache(&mut self, image: Option<egui::ColorImage>) {
        self.neon_menu.set_cached_scene(&self.device, &self.queue, image);
    }

    pub(in crate::renderer) fn set_neon_menu(&mut self, frame: Option<MenuFrame>) {
        if frame.is_none() {
            // Leaving the page: next visit replays the power-on.
            self.neon_menu.anim = Anim::default();
            self.neon_menu.last_tick = None;
        }
        self.neon_menu.frame = frame;
    }

    /// The renderer-drawn main page is wanted: frontend Main, nothing on top
    /// that owns the screen.
    pub(in crate::renderer) fn neon_menu_wanted(&self) -> bool {
        self.neon_menu.frame.is_some()
            && self.ui_state.frontend_main_scene
            && self.ui_state.mode != ui::OverlayMode::Video
            && !self.ui_state.video.skip_ui
            && self.ui_state.loading.is_none()
    }

    pub(in crate::renderer) fn neon_menu_ready(&self) -> bool {
        self.neon_menu.ready()
    }

    pub(in crate::renderer) fn prepare_neon_menu(&mut self) {
        let mut menu = std::mem::take(&mut self.neon_menu);
        menu.prepare(self);
        self.neon_menu = menu;
    }

    /// Draw the page into `output`. `plain_scene` copies the bare scene instead,
    /// for the one frame the cached menu photograph is captured.
    pub(in crate::renderer) fn submit_neon_menu(&self, output: &wgpu::TextureView, plain_scene: bool) -> bool {
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("JKA neon main menu"),
        });
        if !self.neon_menu.encode(&mut encoder, output, plain_scene) {
            return false;
        }
        self.queue.submit([encoder.finish()]);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validate(source: &str, name: &str) {
        let module = wgpu::naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|error| panic!("{name}: {}", error.emit_to_string(source)));
        wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(), wgpu::naga::valid::Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|error| panic!("{name}: {}", error.emit_to_string(source)));
    }

    #[test]
    fn neon_menu_shaders_validate() {
        validate(include_str!("../neon_menu/neon_signs.wgsl"), "neon_signs");
        validate(include_str!("../neon_menu/neon_text.wgsl"), "neon_text");
        validate(include_str!("../neon_menu/neon_blur.wgsl"), "neon_blur");
        validate(include_str!("../neon_menu/neon_outside.wgsl"), "neon_outside");
        validate(include_str!("../neon_menu/neon_window.wgsl"), "neon_window");
    }

    #[test]
    fn uniform_sizes_match_wgsl() {
        assert_eq!(std::mem::size_of::<WindowUniform>(), (5 + 18 + 32) * 16);
        assert_eq!(std::mem::size_of::<OutsideUniform>(), (3 + 3 + 1) * 16);
        assert_eq!(std::mem::size_of::<ItemLight>(), 48);
        assert_eq!(std::mem::size_of::<GlyphInstance>(), 32);
        assert_eq!(std::mem::size_of::<text::TextVertex>(), 48);
        assert_eq!(std::mem::size_of::<crate::glass::Uniform>(), 9 * 16);
    }

    #[test]
    fn tv_homography_maps_corners_to_unit_square() {
        let quad = crate::neon_menu::tv_corners(16.0 / 9.0);
        let m = screen_to_quad(&quad);
        let m = glam::Mat3::from_cols_array_2d(&m);
        for (corner, expected) in quad.iter().zip([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]) {
            let q = m * glam::Vec3::new(corner[0], corner[1], 1.0);
            assert!((q.x / q.z - expected[0]).abs() < 1e-4 && (q.y / q.z - expected[1]).abs() < 1e-4, "{corner:?} -> {q:?}");
        }
    }

    #[test]
    fn saber_stays_within_the_title() {
        let layout = crate::neon_menu::layout::compute(1920.0, 1080.0).expect("layout");
        let s = saber(&layout, 1920.0, 1080.0);
        let x0 = layout.title.x * 1920.0;
        let x1 = x0 + font::measure(TITLE) * layout.title.cap * 1080.0;
        let left = s.hilt.iter().flat_map(|l| [l[0][0], l[1][0]]).fold(f32::MAX, f32::min) - s.hilt_tube;
        assert!(left >= x0 - 0.01 && s.blade[1][0] + s.blade_tube <= x1 + 0.01);
    }

    #[test]
    fn every_label_produces_tubes() {
        let layout = crate::neon_menu::layout::compute(1920.0, 1080.0).expect("layout");
        let (prims, instances) = build_glyphs(&layout, 1920.0, 1080.0);
        assert!(!instances.is_empty());
        for instance in &instances {
            let end = (instance.range[0] + instance.range[1]) as usize * 2;
            assert!(end <= prims.len());
            assert!(instance.rect[0] < instance.rect[2] && instance.rect[1] < instance.rect[3]);
        }
    }
}
