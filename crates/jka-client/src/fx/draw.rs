//! View-dependent FX geometry: OpenJK tr_surface.cpp RB_SurfaceSprite,
//! RB_SurfaceOrientedQuad, RB_SurfaceLine (DoLine) and RB_SurfaceCylinder,
//! built on the CPU against the final render view and batched per shader.

use super::system::{make_normal_vectors, FxDraw};
use crate::{
    camera::Camera,
    materials::TextureData,
    renderer::{DynamicModelAlphaMode, DynamicModelSurface, DynamicWireframeClass, DynamicModelVertex, FxGpuSpriteInstance, FxGpuSprites},
    scene,
};
use rayon::prelude::*;
use std::{collections::HashMap, sync::{Arc, OnceLock}};

/// Entity number used for FX surfaces (ENTITYNUM_NONE).
const FX_ENTITY_NUM: u16 = 1023;
const NUM_CYLINDER_SEGMENTS: usize = 32;
const MAX_FX_TESS_WORKERS: usize = 8;

static FX_TESS_WORKER_POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();

fn fx_tess_worker_pool() -> Option<&'static rayon::ThreadPool> {
    FX_TESS_WORKER_POOL
        .get_or_init(|| {
            let workers = std::thread::available_parallelism()
                .map_or(1, |count| count.get())
                .saturating_sub(2)
                .clamp(1, MAX_FX_TESS_WORKERS);
            rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .thread_name(|index| format!("jka-fx-geom-{index}"))
                .build()
                .map_err(|error| eprintln!("FX geometry worker pool unavailable: {error}"))
                .ok()
        })
        .as_ref()
}

/// refdef vieworg/viewaxis in JKA space.
#[derive(Clone, Copy, Debug)]
pub struct FxView {
    pub origin: [f32; 3],
    /// forward, left, up.
    pub axis: [[f32; 3]; 3],
    pub fov_x: f32,
    /// CGame presentation time, for authored texture animation.
    pub seconds: f32,
}

impl FxView {
    pub fn from_camera(camera: &Camera) -> Self {
        // Camera stores yaw and the negated JKA pitch in radians.
        let pitch = -camera.pitch;
        let (sp, cp) = pitch.sin_cos();
        let (sy, cy) = camera.yaw.sin_cos();
        let forward = [cp * cy, cp * sy, -sp];
        let left = [-sy, cy, 0.0];
        let up = [sp * cy, sp * sy, cp];
        Self {
            origin: scene::jka_position(camera.position.to_array()),
            axis: [forward, left, up],
            fov_x: 90.0,
            seconds: 0.0,
        }
    }
}

/// How a shader's first stage composites, from its blendFunc.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FxBlend {
    /// GL_ONE GL_ONE: alpha is irrelevant.
    Add,
    /// GL_SRC_ALPHA GL_ONE.
    AddAlpha,
    /// GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA (and premultiplied variants).
    Alpha,
    /// GL_DST_COLOR GL_ZERO / GL_ZERO GL_SRC_COLOR.
    Modulate,
    /// GL_DST_COLOR GL_ONE: src*dst + dst. Used by the stock personal shield.
    DstColorAdd,
    /// GL_DST_COLOR GL_SRC_COLOR: Quake 3/JKA 2x modulation.
    /// RGB = src*dst + dst*src = 2*src*dst. Stock rivetmark uses this.
    Modulate2x,
    /// GL_ZERO GL_ONE_MINUS_SRC_COLOR.
    Darken,
    /// No blendFunc.
    Opaque,
}

impl FxBlend {
    pub fn from_blend_func(blend: &str) -> Self {
        let words: Vec<_> = blend.split_whitespace().map(str::to_ascii_lowercase).collect();
        match words.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            ["add"] | ["gl_one", "gl_one"] => Self::Add,
            ["gl_src_alpha", "gl_one"] => Self::AddAlpha,
            // Stock personalshield chrome stage. Preserve the fixed-function
            // equation exactly: source*destination + destination.
            ["gl_dst_color", "gl_one"] => Self::DstColorAdd,
            ["filter"] | ["gl_dst_color", "gl_zero"] | ["gl_zero", "gl_src_color"] => Self::Modulate,
            ["gl_dst_color", "gl_src_color"] => Self::Modulate2x,
            ["gl_zero", "gl_one_minus_src_color"] => Self::Darken,
            // GL_ONE GL_ZERO replaces the framebuffer: opaque whatever the texture alpha
            // (player entity-tint stages rely on this to keep their alpha mask unblended).
            [] | ["gl_one", "gl_zero"] => Self::Opaque,
            _ => Self::Alpha,
        }
    }

    /// A refEntity customShader drawn over a model: opaque stays opaque.
    pub fn custom_shader_alpha_mode(self) -> DynamicModelAlphaMode {
        match self {
            Self::Opaque => DynamicModelAlphaMode::Opaque,
            other => other.alpha_mode(),
        }
    }

    fn alpha_mode(self) -> DynamicModelAlphaMode {
        match self {
            Self::Add => DynamicModelAlphaMode::AdditiveOne,
            Self::AddAlpha => DynamicModelAlphaMode::Additive,
            Self::Modulate => DynamicModelAlphaMode::Modulate,
            Self::DstColorAdd => DynamicModelAlphaMode::DstColorAdd,
            Self::Modulate2x => DynamicModelAlphaMode::Modulate2x,
            Self::Darken => DynamicModelAlphaMode::Darken,
            Self::Alpha | Self::Opaque => DynamicModelAlphaMode::BlendUnlit,
        }
    }
}

/// Immutable animation frames loaded once with an FX shader. The stage's
/// current image is chosen at presentation time, not during material caching.
/// This mirrors TaystJK R_BindAnimatedImage without reloading images per frame.
#[derive(Clone, Debug)]
pub struct FxTextureAnimation {
    pub fps: f32,
    pub one_shot: bool,
    pub frames: Vec<Arc<TextureData>>,
}

impl FxTextureAnimation {
    pub fn frame_index(&self, shader_time: f32) -> usize {
        crate::materials::animation_frame_index(self.frames.len(), self.fps, self.one_shot, shader_time)
    }
}

/// The subset of a shader's primary stage an FX sprite needs.
#[derive(Clone, Debug)]
pub struct FxMaterial {
    pub texture: Option<Arc<TextureData>>,
    pub animation: Option<FxTextureAnimation>,
    pub blend: FxBlend,
    /// rgbGen vertex/exactVertex (FX shaders normally use vertex color).
    pub rgb_vertex: bool,
    pub alpha_vertex: bool,
    pub rgb_const: [f32; 3],
    pub alpha_const: f32,
    /// Per-stage colour generators evaluated at TaystJK shader time.
    pub rgb_wave: Option<jka_assets::shader::Wave>,
    pub alpha_wave: Option<jka_assets::shader::Wave>,
    /// oneMinusVertex inverts the vertex RGB/alpha; it is not vertex itself.
    pub rgb_one_minus_vertex: bool,
    pub alpha_one_minus_vertex: bool,
    pub tc_mods: Vec<jka_assets::shader::TcMod>,
    /// OpenJK shader_t::timeOffset, subtracted from refdef time.
    pub shader_time_offset: f32,
    pub shader_clamp_time: Option<f32>,
    /// Exact JKA shader-stage `glow` marker.
    pub authored_glow: bool,
}

/// One authored shader stage drawn in OpenJK's 640x480 virtual 2D space.
/// Used for CGame effects such as CG_SaberClashFlare that are deliberately
/// screen-space rather than world-space sprites.
#[derive(Clone, Debug)]
pub struct ScreenFxDraw {
    /// x, y, width, height in the 640x480 virtual CGame coordinate system.
    pub rect: [f32; 4],
    /// u0, v0, u1, v1. Full-material draws use [0, 0, 1, 1]; atlas-backed
    /// CGame HUD elements (for example the reward count charset) can crop.
    pub uv_rect: [f32; 4],
    pub color: [f32; 4],
    pub material: FxMaterial,
    pub anchor: ScreenFxAnchor,
}

/// How a `ScreenFxDraw` rect follows the screen aspect ratio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenFxAnchor {
    /// Stretch the 640x480 space over the whole screen.
    Stretch,
    /// CGame's `widthRatioCoef` rule around SCREEN_WIDTH / 2. Used by
    /// centered legacy HUD art such as reward medals.
    Center,
    /// CGame's `widthRatioCoef` HUD rule: keep the 4:3 proportions and the
    /// distance from the right edge (icons drawn at `SCREEN_WIDTH - k`).
    Right,
}

impl FxMaterial {
    fn shader_time(&self, seconds: f32) -> f32 {
        jka_assets::shader::shader_time(seconds, self.shader_time_offset, self.shader_clamp_time)
    }

    /// Select the animation frame using the shader's *own* time origin.
    /// No work is performed for the overwhelmingly common static stage.
    pub fn texture_at_time(&self, seconds: f32) -> Option<Arc<TextureData>> {
        if let Some(animation) = &self.animation {
            if let Some(texture) = animation.frames.get(animation.frame_index(self.shader_time(seconds))) {
                return Some(Arc::clone(texture));
            }
        }
        self.texture.clone()
    }

    /// shaderRGBA through rgbGen/alphaGen to the vertex color the dynamic
    /// pipeline multiplies with the texture.
    fn vertex_color(&self, rgba: [u8; 4], seconds: f32) -> [f32; 4] {
        self.float_vertex_color_at_time([
            f32::from(rgba[0]) / 255.0,
            f32::from(rgba[1]) / 255.0,
            f32::from(rgba[2]) / 255.0,
            f32::from(rgba[3]) / 255.0,
        ], seconds)
    }

    /// The screen-space UI fallback has no shader clock; authored FX geometry
    /// uses `float_vertex_color_at_time` with the CGame view's time instead.
    pub(crate) fn float_vertex_color(&self, rgba: [f32; 4]) -> [f32; 4] {
        self.float_vertex_color_at_time(rgba, 0.0)
    }

    fn float_vertex_color_at_time(&self, rgba: [f32; 4], seconds: f32) -> [f32; 4] {
        let seconds = self.shader_time(seconds);
        let mut rgb = if self.rgb_vertex {
            if self.rgb_one_minus_vertex {
                [1.0 - rgba[0], 1.0 - rgba[1], 1.0 - rgba[2]]
            } else {
                [rgba[0], rgba[1], rgba[2]]
            }
        } else {
            self.rgb_const
        };
        if let Some(wave) = self.rgb_wave {
            let value = (wave.base + wave.amplitude *
                fx_wave(wave.func, fx_wave_phase(wave, seconds))).clamp(0.0, 1.0);
            rgb = [value; 3];
        }
        let generated_alpha = if let Some(wave) = self.alpha_wave {
            (wave.base + wave.amplitude *
                fx_wave(wave.func, fx_wave_phase(wave, seconds))).clamp(0.0, 1.0)
        } else if self.alpha_vertex {
            if self.alpha_one_minus_vertex { 1.0 - rgba[3] } else { rgba[3] }
        } else {
            self.alpha_const
        };
        let alpha = match self.blend {
            // GL_ONE-style stages ignore source alpha in their blend equation.
            FxBlend::Add
            | FxBlend::Modulate
            | FxBlend::DstColorAdd
            | FxBlend::Modulate2x
            | FxBlend::Darken => 1.0,
            _ => generated_alpha,
        };
        [rgb[0], rgb[1], rgb[2], alpha]
    }
}

#[derive(Default)]
struct Batch {
    vertices: Vec<DynamicModelVertex>,
    indices: Vec<u32>,
}

impl Batch {
    fn vertex(&mut self, jka: [f32; 3], uv: [f32; 2], color: [f32; 4]) -> u32 {
        let index = self.vertices.len() as u32;
        self.vertices.push(DynamicModelVertex {
            position: scene::render_position(jka),
            normal: [0.0, 1.0, 0.0],
            uv,
            color,
            depth_hack: 0.0,
            water_wetness: 0.0,
        });
        index
    }

    /// Both windings: FX surfaces are two-sided (cull none) in OpenJK.
    fn triangle(&mut self, a: u32, b: u32, c: u32) {
        self.indices.extend_from_slice(&[a, b, c, a, c, b]);
    }

    /// RB_AddQuadStamp.
    fn quad_stamp(&mut self, origin: [f32; 3], left: [f32; 3], up: [f32; 3], color: [f32; 4]) {
        let v0 = self.vertex(add(add(origin, left), up), [0.0, 0.0], color);
        let v1 = self.vertex(add(sub(origin, left), up), [1.0, 0.0], color);
        let v2 = self.vertex(sub(sub(origin, left), up), [1.0, 1.0], color);
        let v3 = self.vertex(sub(add(origin, left), up), [0.0, 1.0], color);
        self.triangle(v0, v1, v3);
        self.triangle(v3, v1, v2);
    }
}

/// Build per-shader/per-stage FX surfaces for this view. `materials` resolves
/// every renderable stage of a shader. This matters for stock JKA effects such
/// as `gfx/effects/sabers/saberBlur`, whose glow and hot core are two additive
/// stages over the same trail quad.
pub fn tessellate(
    draws: &[FxDraw],
    view: &FxView,
    materials: &mut dyn FnMut(&str) -> Vec<FxMaterial>,
) -> Vec<DynamicModelSurface> {
    let mut batches: HashMap<(String, usize), (FxMaterial, Batch)> = HashMap::new();
    let mut material_cache: HashMap<String, Vec<FxMaterial>> = HashMap::new();
    let mut lit_surfaces: Vec<DynamicModelSurface> = Vec::new();
    for draw in draws {
        let shader = match draw {
            FxDraw::Sprite { shader, .. }
            | FxDraw::SaberGlow { shader, .. }
            | FxDraw::OrientedQuad { shader, .. }
            | FxDraw::Line { shader, .. }
            | FxDraw::Quad { shader, .. }
            | FxDraw::Mesh { shader, .. }
            | FxDraw::Cylinder { shader, .. }
            | FxDraw::Billboard { shader, .. } => shader,
            FxDraw::LitMesh { .. } => {
                lit_surfaces.extend(lit_mesh_surface(draw));
                continue;
            }
        };
        let shader_key = shader.to_ascii_lowercase();
        let stages = material_cache
            .entry(shader_key.clone())
            .or_insert_with(|| materials(shader));
        for (stage_index, mat) in stages.iter().enumerate() {
            let entry = batches
                .entry((shader_key.clone(), stage_index))
                .or_insert_with(|| (mat.clone(), Batch::default()));
            append_draw(draw, view, mat, &mut entry.1);
        }
    }
    let mut surfaces: Vec<_> = batches
        .into_iter()
        .filter(|(_, (_, batch))| !batch.indices.is_empty())
        .map(|(_, (mat, batch))| DynamicModelSurface {
            geometry_source: crate::renderer::ModelGeometrySource::Fx,
            authored_lighting: false,
            material_trace: None,
            entity_num: FX_ENTITY_NUM,
            wireframe_class: DynamicWireframeClass::Effect,
            raster_visible: true,
            vertices: Arc::new(batch.vertices),
            indices: Arc::new(batch.indices),
            lighting_origin: None,
            rt_rigid: None,
            rt_skinned_key: None,
            ghoul2_gpu: None,
            fx_gpu_sprites: None,
            texture: mat.texture_at_time(view.seconds),
            alpha_mode: mat.blend.alpha_mode(),
            authored_glow: mat.authored_glow,
        })
        .collect();
    surfaces.extend(lit_surfaces);
    // Deterministic submission order across frames.
    surfaces.sort_by_key(|surface| (surface.alpha_mode as u8, surface.vertices.len()));
    surfaces
}

/// Same output as [`tessellate`], but the expensive view-dependent geometry
/// expansion is distributed across Rayon workers. Material lookup remains on
/// the presentation thread because EntityPresenter owns the texture/shader
/// caches; workers receive only immutable resolved materials and draw indices.
///
/// This is intentionally an A/B path, not a semantic rewrite: draw order inside
/// each material batch is preserved and the resulting vertex/index data uses the
/// exact same `append_draw` implementation as the CPU reference path.
pub fn tessellate_workers(
    draws: &[FxDraw],
    view: &FxView,
    materials: &mut dyn FnMut(&str) -> Vec<FxMaterial>,
) -> Vec<DynamicModelSurface> {
    const PARALLEL_MIN_DRAWS: usize = 64;
    const PARALLEL_CHUNK_DRAWS: usize = 64;

    if draws.len() < PARALLEL_MIN_DRAWS {
        return tessellate(draws, view, materials);
    }

    let mut groups: HashMap<(String, usize), (FxMaterial, Vec<usize>)> = HashMap::new();
    let mut material_cache: HashMap<String, Vec<FxMaterial>> = HashMap::new();
    let mut lit_surfaces: Vec<DynamicModelSurface> = Vec::new();

    for (draw_index, draw) in draws.iter().enumerate() {
        let shader = match draw {
            FxDraw::Sprite { shader, .. }
            | FxDraw::SaberGlow { shader, .. }
            | FxDraw::OrientedQuad { shader, .. }
            | FxDraw::Line { shader, .. }
            | FxDraw::Quad { shader, .. }
            | FxDraw::Mesh { shader, .. }
            | FxDraw::Cylinder { shader, .. }
            | FxDraw::Billboard { shader, .. } => shader,
            FxDraw::LitMesh { .. } => {
                lit_surfaces.extend(lit_mesh_surface(draw));
                continue;
            }
        };
        let shader_key = shader.to_ascii_lowercase();
        let stages = material_cache
            .entry(shader_key.clone())
            .or_insert_with(|| materials(shader));
        for (stage_index, mat) in stages.iter().enumerate() {
            groups
                .entry((shader_key.clone(), stage_index))
                .or_insert_with(|| (mat.clone(), Vec::new()))
                .1
                .push(draw_index);
        }
    }

    let Some(pool) = fx_tess_worker_pool() else {
        return tessellate(draws, view, materials);
    };

    let mut surfaces: Vec<_> = pool.install(|| groups
        .into_iter()
        .collect::<Vec<_>>()
        .into_par_iter()
        .filter_map(|(_, (mat, draw_indices))| {
            let batch = if draw_indices.len() < PARALLEL_MIN_DRAWS {
                let mut batch = Batch::default();
                for draw_index in draw_indices {
                    append_draw(&draws[draw_index], view, &mat, &mut batch);
                }
                batch
            } else {
                let chunks: Vec<Batch> = draw_indices
                    .par_chunks(PARALLEL_CHUNK_DRAWS)
                    .map(|chunk| {
                        let mut batch = Batch::default();
                        for &draw_index in chunk {
                            append_draw(&draws[draw_index], view, &mat, &mut batch);
                        }
                        batch
                    })
                    .collect();

                let mut merged = Batch::default();
                for chunk in chunks {
                    append_batch(&mut merged, chunk);
                }
                merged
            };

            (!batch.indices.is_empty()).then(|| DynamicModelSurface {
                geometry_source: crate::renderer::ModelGeometrySource::Fx,
                authored_lighting: false,
                material_trace: None,
                entity_num: FX_ENTITY_NUM,
                wireframe_class: DynamicWireframeClass::Effect,
                raster_visible: true,
                vertices: Arc::new(batch.vertices),
                indices: Arc::new(batch.indices),
                lighting_origin: None,
                rt_rigid: None,
                rt_skinned_key: None,
                ghoul2_gpu: None,
                fx_gpu_sprites: None,
                texture: mat.texture_at_time(view.seconds),
                alpha_mode: mat.blend.alpha_mode(),
                authored_glow: mat.authored_glow,
            })
        })
        .collect());

    surfaces.extend(lit_surfaces);
    surfaces.sort_by_key(|surface| (surface.alpha_mode as u8, surface.vertices.len()));
    surfaces
}

/// Pack an RB_AddQuadStamp-style quad into the existing four vec4 instancing
/// layout. Both sprite and line/tail geometry use exactly this representation.
fn fx_gpu_quad_instance(
    origin: [f32; 3],
    left: [f32; 3],
    up: [f32; 3],
    color: [f32; 4],
) -> FxGpuSpriteInstance {
    let origin = scene::render_position(origin);
    let left = scene::render_position(left);
    let up = scene::render_position(up);
    FxGpuSpriteInstance {
        origin: [origin[0], origin[1], origin[2], 0.0],
        left: [left[0], left[1], left[2], 0.0],
        up: [up[0], up[1], up[2], 0.0],
        color,
    }
}

/// Keep JKA/OpenJK FX simulation semantics on the CPU, but submit common
/// quad-based FX as compact WGPU instances. The billboard basis is evaluated
/// against the final render view so this changes geometry submission cost,
/// not authored motion, lifetime or orientation. GPU-eligible sprites,
/// oriented quads, saber-glow beads and view-facing line/tail strips use this
/// path. Complex geometry and tcMod stages keep the reference CPU tessellation.
pub fn tessellate_gpu_particles(
    draws: &[FxDraw],
    view: &FxView,
    materials: &mut dyn FnMut(&str) -> Vec<FxMaterial>,
) -> Vec<DynamicModelSurface> {
    let mut cpu_batches: HashMap<(String, usize), (FxMaterial, Batch)> = HashMap::new();
    let mut sprite_batches: HashMap<(String, usize), (FxMaterial, Vec<FxGpuSpriteInstance>)> = HashMap::new();
    let mut material_cache: HashMap<String, Vec<FxMaterial>> = HashMap::new();
    let mut lit_surfaces: Vec<DynamicModelSurface> = Vec::new();

    for draw in draws {
        let shader = match draw {
            FxDraw::Sprite { shader, .. }
            | FxDraw::SaberGlow { shader, .. }
            | FxDraw::OrientedQuad { shader, .. }
            | FxDraw::Line { shader, .. }
            | FxDraw::Quad { shader, .. }
            | FxDraw::Mesh { shader, .. }
            | FxDraw::Cylinder { shader, .. }
            | FxDraw::Billboard { shader, .. } => shader,
            FxDraw::LitMesh { .. } => {
                lit_surfaces.extend(lit_mesh_surface(draw));
                continue;
            }
        };
        let shader_key = shader.to_ascii_lowercase();
        let stages = material_cache
            .entry(shader_key.clone())
            .or_insert_with(|| materials(shader));

        for (stage_index, mat) in stages.iter().enumerate() {
            // Compact sprite instances have fixed 0..1 UVs. Animated stages
            // use the shared CPU path so tcMods match all FX backends.
            if !mat.tc_mods.is_empty() {
                let entry = cpu_batches
                    .entry((shader_key.clone(), stage_index))
                    .or_insert_with(|| (mat.clone(), Batch::default()));
                append_draw(draw, view, mat, &mut entry.1);
                continue;
            }
            // Legacy saber glow is a train of camera-facing quad stamps.
            // Emit the same beads as instances instead of expanding each into
            // four CPU vertices and indexed triangles every presentation frame.
            if let FxDraw::SaberGlow { origin, direction, length, radius, hilt_radius, rgba, .. } = draw {
                let instances = &mut sprite_batches
                    .entry((shader_key.clone(), stage_index))
                    .or_insert_with(|| (mat.clone(), Vec::new()))
                    .1;
                let color = mat.vertex_color(*rgba, view.seconds);
                let mut distance = *length;
                let mut glow_radius = *radius;
                while distance > 0.0 {
                    let point = add(*origin, scale(*direction, distance));
                    instances.push(fx_gpu_quad_instance(
                        point, scale(view.axis[1], glow_radius),
                        scale(view.axis[2], glow_radius), color,
                    ));
                    distance -= glow_radius * 0.65;
                    glow_radius += 0.017;
                }
                if *hilt_radius > 0.0 {
                    instances.push(fx_gpu_quad_instance(
                        *origin, scale(view.axis[1], *hilt_radius),
                        scale(view.axis[2], *hilt_radius), color,
                    ));
                }
                continue;
            }
            // The existing six-vertex instanced quad also represents a view-facing
            // line exactly: origin is its midpoint, left its cross-view half-width,
            // and up the half-vector toward the starting endpoint. That preserves
            // RB_SurfaceLine's UVs and winding while eliminating four CPU vertices
            // and six indices per spark/tail. No shader or pipeline variant needed.
            let billboard = match draw {
                FxDraw::Sprite { origin, radius, rotation, rgba, .. } => {
                    let (left, up) = rotated_frame(view.axis[1], view.axis[2], *radius, *rotation);
                    Some((*origin, left, up, *rgba))
                }
                FxDraw::OrientedQuad { origin, axis, radius, rotation, rgba, .. } => {
                    let (left, up) = rotated_frame(axis[1], axis[2], *radius, *rotation);
                    Some((*origin, left, up, *rgba))
                }
                FxDraw::Line { start, end, width, rgba, .. } => {
                    let right = normalize(cross(sub(*start, view.origin), sub(*end, view.origin)));
                    let center = scale(add(*start, *end), 0.5);
                    let left = scale(right, *width);
                    let up = scale(sub(*start, *end), 0.5);
                    Some((center, left, up, *rgba))
                }
                _ => None,
            };
            if let Some((origin, left, up, rgba)) = billboard {
                sprite_batches
                    .entry((shader_key.clone(), stage_index))
                    .or_insert_with(|| (mat.clone(), Vec::new()))
                    .1
                    .push(fx_gpu_quad_instance(origin, left, up, mat.vertex_color(rgba, view.seconds)));
            } else {
                let entry = cpu_batches
                    .entry((shader_key.clone(), stage_index))
                    .or_insert_with(|| (mat.clone(), Batch::default()));
                append_draw(draw, view, mat, &mut entry.1);
            }
        }
    }

    let mut surfaces: Vec<DynamicModelSurface> = cpu_batches
        .into_iter()
        .filter(|(_, (_, batch))| !batch.indices.is_empty())
        .map(|(_, (mat, batch))| DynamicModelSurface {
            geometry_source: crate::renderer::ModelGeometrySource::Fx,
            authored_lighting: false,
            material_trace: None,
            entity_num: FX_ENTITY_NUM,
            wireframe_class: DynamicWireframeClass::Effect,
            raster_visible: true,
            vertices: Arc::new(batch.vertices),
            indices: Arc::new(batch.indices),
            lighting_origin: None,
            rt_rigid: None,
            rt_skinned_key: None,
            ghoul2_gpu: None,
            fx_gpu_sprites: None,
            texture: mat.texture_at_time(view.seconds),
            alpha_mode: mat.blend.alpha_mode(),
            authored_glow: mat.authored_glow,
        })
        .collect();

    surfaces.extend(
        sprite_batches
            .into_iter()
            .filter(|(_, (_, instances))| !instances.is_empty())
            .map(|(_, (mat, instances))| DynamicModelSurface {
                geometry_source: crate::renderer::ModelGeometrySource::Fx,
                authored_lighting: false,
                material_trace: None,
                entity_num: FX_ENTITY_NUM,
                wireframe_class: DynamicWireframeClass::Effect,
                raster_visible: true,
                vertices: Arc::new(Vec::new()),
                indices: Arc::new(Vec::new()),
                lighting_origin: None,
                rt_rigid: None,
                rt_skinned_key: None,
                ghoul2_gpu: None,
                fx_gpu_sprites: Some(FxGpuSprites {
                resident: None, instances: Arc::new(instances), blob_shadow: false }),
                texture: mat.texture_at_time(view.seconds),
                alpha_mode: mat.blend.alpha_mode(),
                authored_glow: mat.authored_glow,
            }),
    );

    surfaces.extend(lit_surfaces);
    surfaces.sort_by_key(|surface| (surface.alpha_mode as u8, surface.vertex_count()));
    surfaces
}

/// Check only the actual stages belonging to one shader, not every possible
/// FX alpha mode. The app uses this to avoid any per-particle CPU interpolation
/// when a warm shader has only one compiled blend variant.
pub(crate) fn resident_material_pipelines_ready(stages: &[FxMaterial], ready: u16) -> bool {
    !stages.is_empty() && stages.iter().all(|stage| {
        ready & (1u16 << stage.blend.alpha_mode().index()) != 0
    })
}

/// Material-validated, stable-slot resident sparks. The CPU copies below are
/// *fallbacks only* while WGPU asynchronously compiles the resident shader;
/// once ready it draws from the 112-byte birth record plus u32 slot IDs.
/// Return the exact source draw indices suppressed from the reference path.
pub fn tessellate_resident_sprites(
    candidates: &[(usize, u32, &FxDraw)],
    frame: Arc<crate::renderer::FxGpuResidentFrame>,
    view: &FxView,
    materials: &mut dyn FnMut(&str) -> Vec<FxMaterial>,
) -> (Vec<DynamicModelSurface>, Vec<usize>, Vec<usize>) {
    type StageBatch = (FxMaterial, Vec<u32>, Vec<FxGpuSpriteInstance>);
    let mut batches: HashMap<(String, usize), StageBatch> = HashMap::new();
    let mut material_cache: HashMap<String, Vec<FxMaterial>> = HashMap::new();
    let mut accepted = Vec::new();
    let mut warm = Vec::new();
    for &(index, slot, draw) in candidates {
        let FxDraw::Sprite { origin, radius, rotation, rgba, shader } = draw else { continue; };
        // A full shader is atomic: partial promotion could silently remove an
        // unsupported second stage (glow/tcMod/animation).
        let key = shader.to_ascii_lowercase();
        let stages = material_cache.entry(key.clone()).or_insert_with(|| materials(shader));
        if stages.is_empty() || stages.iter().any(|mat| {
            crate::fx::gpu_sparks::material_stage_eligible(mat).is_err()
                || mat.authored_glow
                || !matches!(mat.blend,
                    FxBlend::Add | FxBlend::AddAlpha | FxBlend::Alpha)
        }) { continue; }
        // Pipeline readiness is published back from the renderer *after* lazy
        // compilation. In the steady state, there is no reason to build the
        // reference 64-byte camera-facing instance just to discard it later.
        let ready = frame.ready_modes.load(std::sync::atomic::Ordering::Acquire);
        let fallback_needed = stages.iter().any(|mat| {
            ready & (1u16 << mat.blend.alpha_mode().index()) == 0
        });
        if !fallback_needed { warm.push(index); }
        let basis = fallback_needed.then(|| {
            rotated_frame(view.axis[1], view.axis[2], *radius, *rotation)
        });
        for (stage, mat) in stages.iter().enumerate() {
            let entry = batches.entry((key.clone(), stage)).or_insert_with(|| {
                (mat.clone(), Vec::new(), Vec::new())
            });
            entry.1.push(slot);
            if let Some((left, up)) = basis {
                if ready & (1u16 << mat.blend.alpha_mode().index()) == 0 {
                    entry.2.push(fx_gpu_quad_instance(*origin, left, up,
                        mat.vertex_color(*rgba, view.seconds)));
                }
            }
        }
        accepted.push(index);
    }
    let mut surfaces = Vec::with_capacity(batches.len());
    for (_, (mat, slots, fallback_instances)) in batches {
        surfaces.push(DynamicModelSurface {
            geometry_source: crate::renderer::ModelGeometrySource::Fx,
            authored_lighting: false,
            material_trace: None,
            entity_num: FX_ENTITY_NUM,
            wireframe_class: DynamicWireframeClass::Effect,
            raster_visible: true,
            vertices: Arc::new(Vec::new()),
            indices: Arc::new(Vec::new()),
            lighting_origin: None,
            rt_rigid: None,
            rt_skinned_key: None,
            ghoul2_gpu: None,
            fx_gpu_sprites: Some(FxGpuSprites {
                resident: Some(crate::renderer::FxGpuResidentSprites {
                    frame: Arc::clone(&frame), slots: Arc::new(slots),
                }),
                instances: Arc::new(fallback_instances),
                blob_shadow: false,
            }),
            texture: mat.texture_at_time(view.seconds),
            alpha_mode: mat.blend.alpha_mode(),
            authored_glow: false,
        });
    }
    // If the first birth or a demo generation reset occurs while all visual
    // particles are hidden, still ship the delta to persistent GPU storage.
    // This marker draws zero instances and never requests a pipeline.
    if surfaces.is_empty() && (frame.reset || !frame.writes.is_empty()) {
        surfaces.push(DynamicModelSurface {
            geometry_source: crate::renderer::ModelGeometrySource::Fx,
            authored_lighting: false,
            material_trace: None,
            entity_num: FX_ENTITY_NUM,
            wireframe_class: DynamicWireframeClass::Effect,
            raster_visible: true,
            vertices: Arc::new(Vec::new()),
            indices: Arc::new(Vec::new()),
            lighting_origin: None,
            rt_rigid: None,
            rt_skinned_key: None,
            ghoul2_gpu: None,
            fx_gpu_sprites: Some(FxGpuSprites {
                resident: Some(crate::renderer::FxGpuResidentSprites {
                    frame: Arc::clone(&frame), slots: Arc::new(Vec::new()),
                }),
                instances: Arc::new(Vec::new()), blob_shadow: false,
            }),
            texture: None,
            alpha_mode: DynamicModelAlphaMode::BlendUnlit,
            authored_glow: false,
        });
    }
    surfaces.sort_by_key(|surface| surface.alpha_mode as u8);
    (surfaces, accepted, warm)
}

/// A `LitMesh` becomes its own alpha-blended, light-grid-lit surface: it needs
/// real normals and a lighting origin, which the shared unlit FX batches lack.
/// Back-face culling is on in the dynamic pipeline, so one winding is enough.
fn lit_mesh_surface(draw: &FxDraw) -> Option<DynamicModelSurface> {
    let FxDraw::LitMesh { positions, normals, rgba, indices, lighting_origin } = draw else {
        return None;
    };
    if positions.is_empty()
        || positions.len() != normals.len()
        || positions.len() != rgba.len()
        || indices.len() < 3
    {
        return None;
    }
    let vertices: Vec<DynamicModelVertex> = positions
        .iter()
        .zip(normals)
        .zip(rgba)
        .map(|((position, normal), color)| DynamicModelVertex {
            position: scene::render_position(*position),
            normal: scene::render_position(*normal),
            uv: [0.0, 0.0],
            color: [
                f32::from(color[0]) / 255.0,
                f32::from(color[1]) / 255.0,
                f32::from(color[2]) / 255.0,
                f32::from(color[3]) / 255.0,
            ],
            depth_hack: 0.0,
            water_wetness: 0.0,
        })
        .collect();
    let count = vertices.len() as u32;
    let indices: Vec<u32> = indices
        .chunks_exact(3)
        .filter(|t| t.iter().all(|&i| i < count))
        .flatten()
        .copied()
        .collect();
    (!indices.is_empty()).then(|| DynamicModelSurface {
        geometry_source: crate::renderer::ModelGeometrySource::Fx,
        authored_lighting: false,
        material_trace: None,
        entity_num: FX_ENTITY_NUM,
        wireframe_class: DynamicWireframeClass::Effect,
        raster_visible: true,
        vertices: Arc::new(vertices),
        indices: Arc::new(indices),
        lighting_origin: Some(*lighting_origin),
        rt_rigid: None,
        rt_skinned_key: None,
        ghoul2_gpu: None,
        fx_gpu_sprites: None,
        texture: None,
        alpha_mode: DynamicModelAlphaMode::Blend,

        authored_glow: false,
    })
}

fn append_batch(dst: &mut Batch, mut src: Batch) {
    let base = dst.vertices.len() as u32;
    dst.vertices.append(&mut src.vertices);
    dst.indices
        .extend(src.indices.into_iter().map(|index| index.saturating_add(base)));
}

fn append_draw(draw: &FxDraw, view: &FxView, mat: &FxMaterial, batch: &mut Batch) {
    let first_vertex = batch.vertices.len();
    match draw {
        FxDraw::Sprite { origin, radius, rotation, rgba, .. } => {
            let (left, up) = rotated_frame(view.axis[1], view.axis[2], *radius, *rotation);
            batch.quad_stamp(*origin, left, up, mat.vertex_color(*rgba, view.seconds));
        }
        FxDraw::SaberGlow { origin, direction, length, radius, hilt_radius, rgba, .. } => {
            // TaystJK/OpenJK RB_SurfaceSaberGlow. The renderer receives one
            // RT_SABER_GLOW entity, then walks billboards from blade tip back
            // toward the hilt. Radius grows by exactly 0.017 per blob and the
            // step is exactly 65% of the current radius; do not clamp it here.
            let color = mat.vertex_color(*rgba, view.seconds);
            let mut distance = *length;
            let mut glow_radius = *radius;
            while distance > 0.0 {
                let point = add(*origin, scale(*direction, distance));
                let left = scale(view.axis[1], glow_radius);
                let up = scale(view.axis[2], glow_radius);
                batch.quad_stamp(point, left, up, color);
                distance -= glow_radius * 0.65;
                glow_radius += 0.017;
            }
            // RB_SurfaceSaberGlow always stamps the separate pulsing hilt blob.
            if *hilt_radius > 0.0 {
                let left = scale(view.axis[1], *hilt_radius);
                let up = scale(view.axis[2], *hilt_radius);
                batch.quad_stamp(*origin, left, up, color);
            }
        }
        FxDraw::Billboard { origin, half, rotation, uv, rgba, align, facing, ground, pivot, .. } => {
            let color = mat.vertex_color(*rgba, view.seconds);
            let (left, up) = match (facing, align) {
                // Custom facing: quad in the plane perpendicular to the facing
                // vector; long axis from the alignment, else rotation about it.
                (Some(normal), _) => {
                    let n = normalize(*normal);
                    let seed = match align {
                        Some(d) if dot(*d, n).abs() < 0.99 => *d,
                        _ => if n[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] },
                    };
                    let up0 = normalize(sub(seed, scale(n, dot(seed, n))));
                    let left0 = cross(n, up0);
                    let (sin, cos) = if align.is_some() { (0.0, 1.0) } else { rotation.to_radians().sin_cos() };
                    let left = add(scale(left0, cos), scale(up0, sin));
                    let up = sub(scale(up0, cos), scale(left0, sin));
                    (scale(left, half[0]), scale(up, half[1]))
                }
                (None, align) => match align {
                // Velocity aligned: long axis along the screen-projected direction.
                Some(direction) => {
                    let to_view = normalize(sub(view.origin, *origin));
                    let along = sub(*direction, scale(to_view, dot(*direction, to_view)));
                    let up = normalize(along);
                    let left = normalize(cross(to_view, up));
                    (scale(left, half[0]), scale(up, half[1]))
                }
                None => {
                    let (sin, cos) = rotation.to_radians().sin_cos();
                    let (l, u) = (view.axis[1], view.axis[2]);
                    let left = add(scale(l, cos), scale(u, sin));
                    let up = sub(scale(u, cos), scale(l, sin));
                    (scale(left, half[0]), scale(up, half[1]))
                }
                },
            };
            // Put the pivot UV on the particle: +left is u=0, +up is v=0.
            let center = sub(sub(*origin, scale(left, 1.0 - 2.0 * pivot[0])), scale(up, 1.0 - 2.0 * pivot[1]));
            billboard_quad(batch, center, left, up, *uv, color, *ground);
        }
        FxDraw::OrientedQuad { origin, axis, radius, rotation, rgba, .. } => {
            let (left, up) = rotated_frame(axis[1], axis[2], *radius, *rotation);
            batch.quad_stamp(*origin, left, up, mat.vertex_color(*rgba, view.seconds));
        }
        FxDraw::Line { start, end, width, rgba, .. } => {
            // RB_SurfaceLine: right = normalize((start-view) x (end-view)).
            let right = normalize(cross(sub(*start, view.origin), sub(*end, view.origin)));
            let color = mat.vertex_color(*rgba, view.seconds);
            let v0 = batch.vertex(add(*start, scale(right, *width)), [0.0, 0.0], color);
            let v1 = batch.vertex(sub(*start, scale(right, *width)), [1.0, 0.0], color);
            let v2 = batch.vertex(add(*end, scale(right, *width)), [0.0, 1.0], color);
            let v3 = batch.vertex(sub(*end, scale(right, *width)), [1.0, 1.0], color);
            batch.triangle(v0, v1, v2);
            batch.triangle(v2, v1, v3);
        }
        FxDraw::Quad { positions, uvs, rgba, .. } => {
            let color = mat.vertex_color(*rgba, view.seconds);
            let v0 = batch.vertex(positions[0], uvs[0], color);
            let v1 = batch.vertex(positions[1], uvs[1], color);
            let v2 = batch.vertex(positions[2], uvs[2], color);
            let v3 = batch.vertex(positions[3], uvs[3], color);
            // Saber/FX shaders commonly specify cull twosided. Dynamic FX
            // batches do not currently carry shader cull state, so emit the
            // reverse winding as well, matching the existing two-sided FX path.
            batch.triangle(v0, v1, v3);
            batch.triangle(v3, v1, v2);
            batch.triangle(v3, v1, v0);
            batch.triangle(v2, v1, v3);
        }
        FxDraw::Mesh { positions, uvs, rgba, indices, .. } => {
            if positions.is_empty() || positions.len() != uvs.len() || positions.len() != rgba.len() {
                return;
            }
            let base = batch.vertices.len() as u32;
            for ((position, uv), color) in positions.iter().zip(uvs).zip(rgba) {
                batch.vertex(*position, *uv, mat.vertex_color(*color, view.seconds));
            }
            for triangle in indices.chunks_exact(3) {
                let [a, b, c] = [triangle[0], triangle[1], triangle[2]];
                if a < positions.len() as u32 && b < positions.len() as u32 && c < positions.len() as u32 {
                    batch.triangle(base + a, base + b, base + c);
                }
            }
        }
        // Built into its own lit surface by the tessellators, never batched.
        FxDraw::LitMesh { .. } => {}
        FxDraw::Cylinder { start, end, axis, start_radius, end_radius, rgba, .. } => {
            cylinder(batch, *start, *end, *axis, *start_radius, *end_radius, mat.vertex_color(*rgba, view.seconds), view);
        }
    }
    for modifier in &mat.tc_mods {
        for vertex in &mut batch.vertices[first_vertex..] {
            vertex.uv = modified_uv(
                vertex.uv,
                vertex.position,
                *modifier,
                mat.shader_time(view.seconds),
            );
        }
    }
}

// Evaluate authored stretch waveforms on the FX CPU vertex path.
fn fx_wave_phase(wave: jka_assets::shader::Wave, seconds: f32) -> f32 {
    if matches!(wave.func, jka_assets::shader::WaveFunc::Noise) {
        (seconds + wave.phase) * wave.frequency
    } else {
        wave.phase + seconds * wave.frequency
    }
}

fn fx_wave(kind: jka_assets::shader::WaveFunc, phase: f32) -> f32 {
    use jka_assets::shader::WaveFunc;
    let x = phase.rem_euclid(1.0);
    match kind {
        WaveFunc::Sin => (x * std::f32::consts::TAU).sin(),
        WaveFunc::Triangle => if x < 0.25 { 4.0*x } else if x < 0.75 { 2.0 - 4.0*x } else { 4.0*x - 4.0 },
        WaveFunc::Square => if x < 0.5 { 1.0 } else { -1.0 },
        WaveFunc::Sawtooth => x,
        WaveFunc::InverseSawtooth => 1.0-x,
        WaveFunc::Noise => { // Smooth deterministic GF_NOISE approximation.
            let n = phase.floor();
            let t = x*x*(3.0 - 2.0*x);
            let noise = |i: f32| ((i * 127.1).sin() * 43758.547).fract().rem_euclid(1.0)*2.0-1.0;
            noise(n) * (1.0-t) + noise(n+1.0)*t
        }
    }
}

pub(crate) fn modified_uv(uv: [f32; 2], position: [f32; 3], modifier: jka_assets::shader::TcMod, seconds: f32) -> [f32; 2] {
    use jka_assets::shader::TcMod;
    match modifier {
        TcMod::Scale(x, y) => [uv[0] * x, uv[1] * y],
        TcMod::Scroll(x, y) => {
            let scroll = [x * seconds, y * seconds];
            [uv[0] + scroll[0] - scroll[0].floor(), uv[1] + scroll[1] - scroll[1].floor()]
        }
        TcMod::Rotate(degrees) => {
            let (sin, cos) = (-degrees * seconds).to_radians().sin_cos();
            let centered = [uv[0] - 0.5, uv[1] - 0.5];
            [centered[0] * cos - centered[1] * sin + 0.5,
             centered[0] * sin + centered[1] * cos + 0.5]
        }
        TcMod::Transform(m) => [uv[0] * m[0] + uv[1] * m[2] + m[4], uv[0] * m[1] + uv[1] * m[3] + m[5]],
        TcMod::Stretch(wave) => {
            let value = wave.base + wave.amplitude * fx_wave(wave.func, fx_wave_phase(wave, seconds));
            let scale = if value.abs() <= 1.0e-6 { 1.0 } else { 1.0 / value };
            [(uv[0] - 0.5) * scale + 0.5, (uv[1] - 0.5) * scale + 0.5]
        }
        TcMod::Turb { amplitude, phase, frequency, .. } => {
            let position = scene::jka_position(position);
            let phase = phase + seconds * frequency;
            let tau = std::f32::consts::TAU;
            [uv[0] + (((position[0] + position[2]) / 1024.0 + phase) * tau).sin() * amplitude,
             uv[1] + ((position[1] / 1024.0 + phase) * tau).sin() * amplitude]
        }
    }
}

/// RB_SurfaceSprite/OrientedQuad rotation of the left/up frame.
/// Quad corners at (+-left, +-up). With a floor height, a quad that reaches
/// the floor is tessellated and each vertex's alpha fades with its height
/// above it: a CPU stand-in for soft particles against the ground plane,
/// which is where emote effects meet scene depth.
fn billboard_quad(batch: &mut Batch, origin: [f32; 3], left: [f32; 3], up: [f32; 3], uv: [f32; 4], color: [f32; 4], ground: Option<f32>) {
    let low = origin[2] - left[2].abs() - up[2].abs();
    let fade = ground.map(|g| (g, (len(left).max(len(up)) * 0.6).clamp(2.0, 24.0)));
    let Some((ground, fade)) = fade.filter(|(g, f)| low < g + f) else {
        let v0 = batch.vertex(add(add(origin, left), up), [uv[0], uv[1]], color);
        let v1 = batch.vertex(add(sub(origin, left), up), [uv[2], uv[1]], color);
        let v2 = batch.vertex(sub(sub(origin, left), up), [uv[2], uv[3]], color);
        let v3 = batch.vertex(sub(add(origin, left), up), [uv[0], uv[3]], color);
        batch.triangle(v0, v1, v3);
        batch.triangle(v3, v1, v2);
        return;
    };
    const N: usize = 5;
    let mut ids = [[0u32; N + 1]; N + 1];
    for (row, line) in ids.iter_mut().enumerate() {
        let t = row as f32 / N as f32;
        for (col, id) in line.iter_mut().enumerate() {
            let s = col as f32 / N as f32;
            let p = add(add(origin, scale(left, 1.0 - 2.0 * s)), scale(up, 1.0 - 2.0 * t));
            let k = ((p[2] - ground) / fade).clamp(0.0, 1.0);
            let k = k * k * (3.0 - 2.0 * k);
            let tex = [uv[0] + (uv[2] - uv[0]) * s, uv[1] + (uv[3] - uv[1]) * t];
            *id = batch.vertex(p, tex, [color[0], color[1], color[2], color[3] * k]);
        }
    }
    for row in 0..N {
        for col in 0..N {
            let (a, b, c, d) = (ids[row][col], ids[row][col + 1], ids[row + 1][col + 1], ids[row + 1][col]);
            batch.triangle(a, b, d);
            batch.triangle(d, b, c);
        }
    }
}

fn len(v: [f32; 3]) -> f32 {
    dot(v, v).sqrt()
}

fn rotated_frame(left_axis: [f32; 3], up_axis: [f32; 3], radius: f32, rotation: f32) -> ([f32; 3], [f32; 3]) {
    if rotation == 0.0 {
        return (scale(left_axis, radius), scale(up_axis, radius));
    }
    let (s, c) = (std::f32::consts::PI * rotation / 180.0).sin_cos();
    let left = add(scale(left_axis, c * radius), scale(up_axis, -s * radius));
    let up = add(scale(up_axis, c * radius), scale(left_axis, s * radius));
    (left, up)
}

/// RB_SurfaceCylinder: `start_radius` ring at start (e->origin, size2),
/// `end_radius` ring at end (e->oldorigin, size1); segment count by distance.
#[allow(clippy::too_many_arguments)]
fn cylinder(
    batch: &mut Batch,
    start: [f32; 3],
    end: [f32; 3],
    axis: [f32; 3],
    start_radius: f32,
    end_radius: f32,
    color: [f32; 4],
    view: &FxView,
) {
    let midpoint = scale(add(start, end), 0.5);
    let length = length(sub(midpoint, view.origin)) * (view.fov_x / 90.0);
    let detail = 1.0 - length / 1024.0;
    let segments = ((NUM_CYLINDER_SEGMENTS as f32 * detail) as usize).clamp(8, NUM_CYLINDER_SEGMENTS);
    let (_right, up) = make_normal_vectors(axis);
    let step = 360.0 / segments as f32;
    let ring = |center: [f32; 3], radius: f32, i: usize| add(center, rotate_around(axis, scale(up, radius), step * i as f32));
    let st_step = 1.0 / segments as f32;
    for i in 0..segments {
        let next = (i + 1) % segments;
        let v0 = batch.vertex(ring(start, start_radius, i), [st_step * i as f32, 1.0], color);
        let v1 = batch.vertex(ring(end, end_radius, i), [st_step * i as f32, 0.0], color);
        let v2 = batch.vertex(ring(end, end_radius, next), [st_step * (i + 1) as f32, 0.0], color);
        let v3 = batch.vertex(ring(start, start_radius, next), [st_step * (i + 1) as f32, 1.0], color);
        batch.triangle(v0, v1, v2);
        batch.triangle(v2, v3, v0);
    }
}

fn rotate_around(dir: [f32; 3], point: [f32; 3], degrees: f32) -> [f32; 3] {
    let (s, c) = degrees.to_radians().sin_cos();
    let d = normalize(dir);
    let k_cross_p = cross(d, point);
    let k_dot_p = dot(d, point);
    std::array::from_fn(|i| point[i] * c + k_cross_p[i] * s + d[i] * k_dot_p * (1.0 - c))
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + b[i])
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    a.map(|v| v * s)
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}
fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = length(a);
    if l > 0.0 { scale(a, 1.0 / l) } else { a }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stretch_uses_phase_and_scales_about_the_texture_center() {
        use jka_assets::shader::{TcMod, Wave, WaveFunc};
        let stretch = TcMod::Stretch(Wave {
            func: WaveFunc::Sin, base: 1.0, amplitude: 1.0,
            phase: 0.25, frequency: 1.0,
        });
        assert_eq!(modified_uv([0.0, 1.0], [0.0; 3], stretch, 0.0), [0.25, 0.75]);
        assert_eq!(modified_uv([0.5, 0.5], [0.0; 3], stretch, 0.0), [0.5, 0.5]);
        // A waveform crossing zero must not inject NaNs into FX geometry.
        assert_eq!(modified_uv([0.0, 1.0], [0.0; 3], stretch, 0.5), [0.0, 1.0]);
    }

    fn white(blend: FxBlend) -> FxMaterial {
        FxMaterial { animation: None, texture: None, blend, rgb_vertex: true, alpha_vertex: true, rgb_const: [1.0; 3], alpha_const: 1.0, rgb_wave: None, alpha_wave: None, rgb_one_minus_vertex: false, alpha_one_minus_vertex: false, tc_mods: Vec::new(), shader_time_offset: 0.0, shader_clamp_time: None, authored_glow: false }
    }

    #[test]
    fn authored_fx_rgb_and_alpha_waves_use_shader_time() {
        use jka_assets::shader::{Wave, WaveFunc};
        let wave = Wave { func: WaveFunc::Sin, base: 0.5, amplitude: 0.5, phase: 0.0, frequency: 1.0 };
        let material = FxMaterial {
            rgb_vertex: false, alpha_vertex: false,
            rgb_wave: Some(wave), alpha_wave: Some(wave),
            shader_time_offset: 0.25,
            ..white(FxBlend::AddAlpha)
        };
        let low = material.vertex_color([255, 0, 0, 255], 0.0);
        let high = material.vertex_color([255, 0, 0, 255], 0.5);
        assert!((low[0] - 0.0).abs() < 1e-5);
        assert!((low[3] - 0.0).abs() < 1e-5);
        assert!((high[0] - 1.0).abs() < 1e-5);
        assert!((high[3] - 1.0).abs() < 1e-5);
        let clamped = FxMaterial { shader_clamp_time: Some(0.25), ..material };
        assert_eq!(clamped.vertex_color([255; 4], 0.5), clamped.vertex_color([255; 4], 9.0));
    }

    #[test]
    fn authored_fx_one_minus_vertex_is_not_plain_vertex() {
        let material = FxMaterial {
            rgb_one_minus_vertex: true, alpha_one_minus_vertex: true,
            ..white(FxBlend::Alpha)
        };
        let color = material.vertex_color([64, 128, 255, 192], 0.0);
        assert!((color[0] - (1.0 - 64.0/255.0)).abs() < 1e-6);
        assert!((color[1] - (1.0 - 128.0/255.0)).abs() < 1e-6);
        assert_eq!(color[2], 0.0);
        assert!((color[3] - (1.0 - 192.0/255.0)).abs() < 1e-6);
    }

    #[test]
    fn fx_wave_generators_match_on_cpu_and_gpu_sprite_submission() {
        use jka_assets::shader::{Wave, WaveFunc};
        let wave = Wave { func: WaveFunc::Triangle, base: 0.5, amplitude: 0.5, phase: 0.0, frequency: 1.0 };
        let material = FxMaterial {
            rgb_wave: Some(wave), alpha_wave: Some(wave),
            shader_time_offset: 0.25, shader_clamp_time: Some(0.25),
            ..white(FxBlend::AddAlpha)
        };
        let draws = [FxDraw::Sprite { origin: [100.0, 0.0, 0.0], radius: 2.0, rotation: 0.0,
            rgba: [64, 128, 192, 255], shader: "test".into() }];
        let camera = FxView { seconds: 3.0, ..view() };
        let cpu = tessellate(&draws, &camera, &mut |_| vec![material.clone()]);
        let gpu = tessellate_gpu_particles(&draws, &camera, &mut |_| vec![material.clone()]);
        let gpu = &gpu[0].fx_gpu_sprites.as_ref().unwrap().instances[0];
        assert_eq!(cpu[0].vertices[0].color, [1.0; 4]);
        assert_eq!(gpu.color, cpu[0].vertices[0].color);
        let noise = Wave { func: WaveFunc::Noise, phase: 0.25, frequency: 2.0, ..wave };
        assert_eq!(fx_wave_phase(noise, 1.0), 2.5);
    }

    #[test]
    fn fx_clamptime_freezes_wave_color_and_tcmod_timeline() {
        use jka_assets::shader::{TcMod, Wave, WaveFunc};
        let wave = Wave {
            func: WaveFunc::Sin, base: 0.5, amplitude: 0.5,
            phase: 0.0, frequency: 1.0,
        };
        let mat = FxMaterial {
            rgb_vertex: false,
            rgb_wave: Some(wave),
            shader_time_offset: 2.0,
            shader_clamp_time: Some(0.25),
            tc_mods: vec![TcMod::Scroll(1.0, 0.0)],
            ..white(FxBlend::Alpha)
        };
        let at_limit = mat.vertex_color([255; 4], 2.25);
        let after_limit = mat.vertex_color([255; 4], 50.0);
        assert_eq!(at_limit, after_limit);
        assert!((at_limit[0] - 1.0).abs() < 1e-6);
        let uv1 = modified_uv([0.0, 0.0], [0.0; 3], mat.tc_mods[0], mat.shader_time(2.25));
        let uv2 = modified_uv([0.0, 0.0], [0.0; 3], mat.tc_mods[0], mat.shader_time(50.0));
        assert_eq!(uv1, uv2);
    }


    fn view() -> FxView {
        FxView { origin: [0.0; 3], axis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], fov_x: 90.0, seconds: 0.0 }
    }

    #[test]
    fn fx_animmap_frame_selection_matches_taystjk_and_remap_time() {
        let frames: Vec<Arc<TextureData>> = (0..4).map(|_| {
            Arc::new(crate::materials::missing_texture_data())
        }).collect();
        let looping = FxTextureAnimation {
            fps: 10.0,
            one_shot: false,
            frames: frames.clone(),
        };
        assert_eq!(looping.frame_index(0.0), 0);
        assert_eq!(looping.frame_index(0.099), 0);
        assert_eq!(looping.frame_index(0.100), 1);
        assert_eq!(looping.frame_index(0.399), 3);
        assert_eq!(looping.frame_index(0.400), 0);
        assert_eq!(looping.frame_index(-3.0), 0);
        let mat = FxMaterial {
            animation: Some(looping),
            shader_time_offset: 2.0,
            ..white(FxBlend::Alpha)
        };
        let frame = mat.texture_at_time(2.35).unwrap();
        assert!(Arc::ptr_eq(&frame, &frames[3]));
        let first = mat.texture_at_time(1.0).unwrap();
        assert!(Arc::ptr_eq(&first, &frames[0]));
        let once = FxTextureAnimation { fps: 10.0, one_shot: true, frames };
        assert_eq!(once.frame_index(3.0), 3);
        let frozen = FxMaterial { shader_clamp_time: Some(0.15), animation: Some(once.clone()), ..white(FxBlend::Alpha) };
        assert_eq!(frozen.animation.as_ref().unwrap().frame_index(frozen.shader_time(100.0)), 1);
        assert_eq!(once.frame_index(f32::NAN), 0);
        assert_eq!(FxTextureAnimation { fps: 0.0, ..once.clone() }.frame_index(1.0), 0);
        let empty = FxMaterial {
            texture: Some(Arc::clone(&once.frames[0])),
            animation: Some(FxTextureAnimation { frames: Vec::new(), ..once }),
            ..white(FxBlend::Alpha)
        };
        assert!(Arc::ptr_eq(&empty.texture_at_time(1.0).unwrap(), empty.texture.as_ref().unwrap()));
    }

    #[test]
    fn fx_animation_selects_the_same_image_in_all_submission_paths() {
        let frames: Vec<_> = (0..4).map(|_| Arc::new(crate::materials::missing_texture_data())).collect();
        let material = FxMaterial {
            texture: Some(Arc::clone(&frames[0])),
            animation: Some(FxTextureAnimation { fps: 10.0, one_shot: false, frames: frames.clone() }),
            shader_time_offset: 1.0, shader_clamp_time: Some(0.25),
            ..white(FxBlend::Alpha)
        };
        let draw = FxDraw::Sprite { origin: [100.0, 0.0, 0.0], radius: 2.0, rotation: 0.0,
            rgba: [255; 4], shader: "test".into() };
        // Reach the worker implementation rather than its small-input fallback.
        let draws = vec![draw; 64];
        let camera = FxView { seconds: 20.0, ..view() };
        for surfaces in [
            tessellate(&draws, &camera, &mut |_| vec![material.clone()]),
            tessellate_workers(&draws, &camera, &mut |_| vec![material.clone()]),
            tessellate_gpu_particles(&draws, &camera, &mut |_| vec![material.clone()]),
        ] {
            assert!(!surfaces.is_empty());
            for surface in surfaces {
                assert!(Arc::ptr_eq(surface.texture.as_ref().unwrap(), &frames[2]));
            }
        }
    }

    #[test]
    fn blend_funcs_map_to_fx_composites() {
        assert_eq!(FxBlend::from_blend_func("gl_one gl_one"), FxBlend::Add);
        assert_eq!(FxBlend::from_blend_func("GL_SRC_ALPHA GL_ONE"), FxBlend::AddAlpha);
        assert_eq!(FxBlend::from_blend_func("blend"), FxBlend::Alpha);
        assert_eq!(FxBlend::from_blend_func("gl_dst_color gl_zero"), FxBlend::Modulate);
        assert_eq!(FxBlend::from_blend_func("gl_dst_color gl_src_color"), FxBlend::Modulate2x);
        assert_eq!(
            FxBlend::from_blend_func("gl_zero gl_one_minus_src_color"),
            FxBlend::Darken
        );
        assert_eq!(FxBlend::from_blend_func(""), FxBlend::Opaque);
        assert_eq!(FxBlend::from_blend_func("GL_ONE GL_ZERO"), FxBlend::Opaque);
        assert_eq!(
            FxBlend::from_blend_func("GL_DST_COLOR GL_ONE"),
            FxBlend::DstColorAdd
        );
    }

    #[test]
    fn sprite_faces_view_with_radius_half_extent_and_add_ignores_alpha_byte() {
        let draws = [FxDraw::Sprite { origin: [100.0, 0.0, 0.0], radius: 2.0, rotation: 0.0, rgba: [255, 128, 0, 0], shader: "a".into() }];
        let surfaces = tessellate(&draws, &view(), &mut |_| vec![white(FxBlend::Add)]);
        let surface = &surfaces[0];
        assert_eq!(surface.alpha_mode, DynamicModelAlphaMode::AdditiveOne);
        let jka: Vec<_> = surface.vertices.iter().map(|v| scene::jka_position(v.position)).collect();
        assert_eq!(jka[0], [100.0, 2.0, 2.0], "origin + left + up");
        assert_eq!(jka[2], [100.0, -2.0, -2.0]);
        assert_eq!(surface.vertices[0].color, [1.0, 128.0 / 255.0, 0.0, 1.0]);
        assert_eq!(surface.indices.len(), 12, "two triangles, both windings");
    }

    #[test]
    fn saber_glow_matches_jka_bead_chain_and_hilt_blob() {
        let draws = [FxDraw::SaberGlow {
            origin: [0.0, 0.0, 0.0],
            direction: [1.0, 0.0, 0.0],
            length: 10.0,
            radius: 2.0,
            hilt_radius: 5.625,
            rgba: [255; 4],
            shader: "saber".into(),
        }];
        let surfaces = tessellate(&draws, &view(), &mut |_| vec![white(FxBlend::Add)]);
        assert_eq!(surfaces.len(), 1);
        let surface = &surfaces[0];
        // 10-unit blade at radius 2 produces eight RT_SABER_GLOW beads
        // with the exact 0.65 step / +0.017 growth, plus the hilt blob.
        assert_eq!(surface.vertices.len(), 9 * 4);
        assert_eq!(surface.indices.len(), 9 * 12);
        let jka: Vec<_> = surface.vertices.iter().map(|v| scene::jka_position(v.position)).collect();
        assert_eq!(jka[0], [10.0, 2.0, 2.0], "first bead is at the blade tip");
        assert_eq!(jka[8 * 4], [0.0, 5.625, 5.625], "last quad is the hilt pulse");
    }

    #[test]
    fn worker_tessellation_matches_reference_sprite_geometry() {
        let draws = (0..128)
            .map(|index| FxDraw::Sprite {
                origin: [100.0 + index as f32, 0.0, 0.0],
                radius: 2.0,
                rotation: index as f32,
                rgba: [255, 128, 0, 64],
                shader: "a".into(),
            })
            .collect::<Vec<_>>();
        let reference = tessellate(&draws, &view(), &mut |_| vec![white(FxBlend::AddAlpha)]);
        let workers = tessellate_workers(&draws, &view(), &mut |_| vec![white(FxBlend::AddAlpha)]);
        assert_eq!(reference.len(), 1);
        assert_eq!(workers.len(), 1);
        assert_eq!(reference[0].indices.as_slice(), workers[0].indices.as_slice());
        assert_eq!(reference[0].vertices.len(), workers[0].vertices.len());
        for (expected, actual) in reference[0].vertices.iter().zip(workers[0].vertices.iter()) {
            assert_eq!(expected.position, actual.position);
            assert_eq!(expected.normal, actual.normal);
            assert_eq!(expected.uv, actual.uv);
            assert_eq!(expected.color, actual.color);
        }
    }

    #[test]
    fn gpu_particle_path_preserves_sprite_basis_and_material() {
        let draws = [FxDraw::Sprite {
            origin: [100.0, 0.0, 0.0],
            radius: 2.0,
            rotation: 0.0,
            rgba: [255, 128, 0, 64],
            shader: "a".into(),
        }];
        let surfaces = tessellate_gpu_particles(&draws, &view(), &mut |_| vec![white(FxBlend::AddAlpha)]);
        assert_eq!(surfaces.len(), 1);
        assert_eq!(surfaces[0].alpha_mode, DynamicModelAlphaMode::Additive);
        assert!(surfaces[0].vertices.is_empty());
        assert!(surfaces[0].indices.is_empty());
        let sprites = surfaces[0].fx_gpu_sprites.as_ref().expect("GPU sprite surface");
        assert_eq!(sprites.instances.len(), 1);
        let instance = sprites.instances[0];
        assert_eq!(instance.origin[..3], scene::render_position([100.0, 0.0, 0.0]));
        assert_eq!(instance.left[..3], scene::render_position([0.0, 2.0, 0.0]));
        assert_eq!(instance.up[..3], scene::render_position([0.0, 0.0, 2.0]));
        assert_eq!(instance.color, [1.0, 128.0 / 255.0, 0.0, 64.0 / 255.0]);
    }

    #[test]
    fn repeater_ripple_tcmods_animate_both_passes_in_every_geometry_backend() {
        use jka_assets::shader::TcMod;
        let mut view = view();
        view.seconds = 2.0;
        let stages = vec![
            FxMaterial { tc_mods: vec![TcMod::Scale(3.0, 1.0), TcMod::Scroll(0.9, 0.0)], ..white(FxBlend::Modulate2x) },
            FxMaterial { tc_mods: vec![TcMod::Scale(2.0, 1.0), TcMod::Scroll(-0.4, 0.0)], ..white(FxBlend::Modulate2x) },
        ];
        let draw = FxDraw::Cylinder {
            start: [100.0, 0.0, 0.0], end: [100.0, 0.0, 12.0],
            axis: [0.0, 0.0, 1.0], start_radius: 24.0, end_radius: 60.0,
            rgba: [255; 4], shader: "gfx/effects/shock_ripple".into(),
        };
        // More than 64 draws exercises the worker path and its chunk merging.
        let draws = vec![draw; 128];
        for surfaces in [
            tessellate(&draws, &view, &mut |_| stages.clone()),
            tessellate_workers(&draws, &view, &mut |_| stages.clone()),
            tessellate_gpu_particles(&draws, &view, &mut |_| stages.clone()),
        ] {
            assert_eq!(surfaces.len(), 2);
            let mut offsets: Vec<_> = surfaces.iter().map(|s| s.vertices[0].uv[0]).collect();
            offsets.sort_by(f32::total_cmp);
            assert!((offsets[0] - 0.2).abs() < 1e-5);
            assert!((offsets[1] - 0.8).abs() < 1e-5);
            for surface in &surfaces {
                assert_eq!(surface.alpha_mode, DynamicModelAlphaMode::Modulate2x);
                let vertices = &surface.vertices;
                let max_u = vertices.iter().map(|v| v.uv[0]).fold(0.0, f32::max);
                assert!((max_u - vertices[0].uv[0] - if vertices[0].uv[0] > 0.5 { 3.0 } else { 2.0 }).abs() < 1e-5);
                let per_draw = vertices.len() / draws.len();
                assert_eq!(vertices[0].uv, vertices[per_draw].uv, "tcMods apply once per draw");
            }
        }
        let scroll_then_scale = FxMaterial {
            tc_mods: vec![TcMod::Scroll(-0.4, 0.0), TcMod::Scale(2.0, 1.0)],
            ..white(FxBlend::Modulate2x)
        };
        let sprite = FxDraw::Sprite { origin: [100.0, 0.0, 0.0], radius: 2.0, rotation: 0.0, rgba: [255; 4], shader: "animated".into() };
        let cpu = tessellate(&[sprite.clone()], &view, &mut |_| vec![scroll_then_scale.clone()]);
        let gpu = tessellate_gpu_particles(&[sprite], &view, &mut |_| vec![scroll_then_scale.clone()]);
        assert!(gpu[0].fx_gpu_sprites.is_none(), "animated sprites use the UV-capable path");
        assert!((gpu[0].vertices[0].uv[0] - 0.4).abs() < 1e-5, "modifier order is preserved");
        assert_eq!(bytemuck::cast_slice::<_, u8>(&*cpu[0].vertices), bytemuck::cast_slice::<_, u8>(&*gpu[0].vertices));
    }

    #[test]
    fn gpu_lines_and_tails_match_reference_quad_corners_and_uvs() {
        // FX tails are represented as FxDraw::Line after CPU simulation.
        // Test both a diagonal and a horizontal view-facing spark strip.
        for (start, end) in [
            ([100.0, 0.0, 10.0], [110.0, 45.0, -5.0]),
            ([100.0, 0.0, 0.0], [100.0, 50.0, 0.0]),
        ] {
            let draws = [FxDraw::Line {
                start, end, width: 1.5, rgba: [255, 128, 64, 200], shader: "spark".into(),
            }];
            let cpu = tessellate(&draws, &view(), &mut |_| vec![white(FxBlend::AddAlpha)]);
            let gpu = tessellate_gpu_particles(&draws, &view(), &mut |_| vec![white(FxBlend::AddAlpha)]);
            let instance = gpu[0].fx_gpu_sprites.as_ref().expect("instanced line").instances[0];
            assert!(gpu[0].vertices.is_empty());
            assert!(gpu[0].indices.is_empty());
            let corners = [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)];
            let uvs = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]];
            for (index, (sx, sy)) in corners.into_iter().enumerate() {
                let actual: [f32; 3] = std::array::from_fn(|i| {
                    instance.origin[i] + sx * instance.left[i] + sy * instance.up[i]
                });
                for component in 0..3 {
                    assert!((actual[component] - cpu[0].vertices[index].position[component]).abs() < 1e-4);
                }
                assert_eq!(cpu[0].vertices[index].uv, uvs[index]);
                assert_eq!(cpu[0].vertices[index].color, instance.color);
            }
        }
    }

    #[test]
    fn gpu_legacy_saber_glow_beads_match_cpu_positions() {
        let draw = FxDraw::SaberGlow {
            origin: [100.0, 5.0, 6.0], direction: [0.0, 1.0, 0.0],
            length: 24.0, radius: 2.0, hilt_radius: 3.0,
            rgba: [255, 50, 10, 128], shader: "glow".into(),
        };
        let cpu = tessellate(&[draw.clone()], &view(), &mut |_| vec![white(FxBlend::AddAlpha)]);
        let gpu = tessellate_gpu_particles(&[draw], &view(), &mut |_| vec![white(FxBlend::AddAlpha)]);
        let instances = &gpu[0].fx_gpu_sprites.as_ref().expect("glow instances").instances;
        assert!(instances.len() > 2);
        assert_eq!(cpu[0].vertices.len(), instances.len() * 4);
        for (bead, instance) in instances.iter().enumerate() {
            let corners = [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)];
            for (offset, (sx, sy)) in corners.into_iter().enumerate() {
                let actual: [f32; 3] = std::array::from_fn(|i| {
                    instance.origin[i] + sx * instance.left[i] + sy * instance.up[i]
                });
                let expected = cpu[0].vertices[bead * 4 + offset];
                for component in 0..3 {
                    assert!((actual[component] - expected.position[component]).abs() < 1e-4);
                }
                assert_eq!(instance.color, expected.color);
            }
        }
    }

    #[test]
    fn gpu_lines_fall_back_for_shader_tcmods() {
        use jka_assets::shader::TcMod;
        let draw = FxDraw::Line {
            start: [100.0, 0.0, 0.0], end: [100.0, 40.0, 0.0], width: 2.0,
            rgba: [255; 4], shader: "spark".into(),
        };
        let material = FxMaterial { tc_mods: vec![TcMod::Scroll(0.5, 0.25)], ..white(FxBlend::AddAlpha) };
        let cpu = tessellate(&[draw.clone()], &view(), &mut |_| vec![material.clone()]);
        let gpu = tessellate_gpu_particles(&[draw], &view(), &mut |_| vec![material.clone()]);
        assert!(gpu[0].fx_gpu_sprites.is_none());
        assert_eq!(gpu[0].vertices.len(), cpu[0].vertices.len());
        assert_eq!(gpu[0].indices.len(), cpu[0].indices.len());
        for (actual, expected) in gpu[0].vertices.iter().zip(cpu[0].vertices.iter()) {
            assert_eq!(actual.uv, expected.uv);
            assert_eq!(actual.position, expected.position);
        }
    }

    #[test]
    fn line_width_spans_perpendicular_to_view_and_segment() {
        let draws = [FxDraw::Line { start: [100.0, 0.0, 0.0], end: [100.0, 50.0, 0.0], width: 1.5, rgba: [255, 255, 255, 128], shader: "b".into() }];
        let surfaces = tessellate(&draws, &view(), &mut |_| vec![white(FxBlend::AddAlpha)]);
        let jka: Vec<_> = surfaces[0].vertices.iter().map(|v| scene::jka_position(v.position)).collect();
        // (start - view) x (end - view) points along +Z here.
        assert!((jka[0][2] - 1.5).abs() < 1e-4 && (jka[1][2] + 1.5).abs() < 1e-4);
        assert!((surfaces[0].vertices[0].color[3] - 128.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn arbitrary_quad_keeps_uvs_and_multistage_shader_draws_both_passes() {
        let draws = [FxDraw::Quad {
            positions: [[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]],
            uvs: [[0.0, 1.0], [0.0, 0.0], [0.5, 0.0], [0.5, 1.0]],
            rgba: [255, 64, 0, 255],
            shader: "gfx/effects/sabers/saberBlur".into(),
        }];
        let surfaces = tessellate(&draws, &view(), &mut |_| vec![
            white(FxBlend::Add),
            FxMaterial { rgb_vertex: false, ..white(FxBlend::Add) },
        ]);
        assert_eq!(surfaces.len(), 2, "one surface per renderable shader stage");
        assert!(surfaces.iter().all(|surface| surface.indices.len() == 12));
        assert!(surfaces.iter().any(|surface| surface.vertices[2].uv == [0.5, 0.0]));
    }

    #[test]
    fn indexed_mesh_preserves_per_vertex_colour_and_indices() {
        let draws = [FxDraw::Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            rgba: vec![[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]],
            indices: vec![0, 1, 2],
            shader: "mesh".into(),
        }];
        let surfaces = tessellate(&draws, &view(), &mut |_| vec![white(FxBlend::Add)]);
        assert_eq!(surfaces.len(), 1);
        assert_eq!(surfaces[0].vertices.len(), 3);
        assert_eq!(surfaces[0].indices.len(), 6, "mesh triangles are emitted two-sided");
        assert_eq!(surfaces[0].vertices[0].color[..3], [1.0, 0.0, 0.0]);
        assert_eq!(surfaces[0].vertices[1].color[..3], [0.0, 1.0, 0.0]);
    }

    #[test]
    fn batches_by_shader_and_cylinder_rings_use_both_radii() {
        let draws = [
            FxDraw::Sprite { origin: [10.0, 0.0, 0.0], radius: 1.0, rotation: 30.0, rgba: [255; 4], shader: "x".into() },
            FxDraw::Sprite { origin: [20.0, 0.0, 0.0], radius: 1.0, rotation: 0.0, rgba: [255; 4], shader: "X".into() },
            FxDraw::Cylinder { start: [0.0; 3], end: [0.0, 0.0, 10.0], axis: [0.0, 0.0, 1.0], start_radius: 1.0, end_radius: 3.0, rgba: [255; 4], shader: "c".into() },
        ];
        let surfaces = tessellate(&draws, &view(), &mut |_| vec![white(FxBlend::Alpha)]);
        assert_eq!(surfaces.len(), 2);
        let cylinder = surfaces.iter().find(|s| s.vertices.len() > 8).unwrap();
        let radii: Vec<f32> = cylinder
            .vertices
            .iter()
            .map(|v| scene::jka_position(v.position))
            .map(|p| (p[0] * p[0] + p[1] * p[1]).sqrt())
            .collect();
        assert!((radii[0] - 1.0).abs() < 1e-4 && (radii[1] - 3.0).abs() < 1e-4);
    }
}

#[cfg(test)]
mod resident_fx_tests {
    use super::*;

    fn stage() -> FxMaterial {
        FxMaterial {
            texture: None, animation: None, blend: FxBlend::AddAlpha,
            rgb_vertex: true, alpha_vertex: true,
            rgb_const: [1.0; 3], alpha_const: 1.0,
            rgb_wave: None, alpha_wave: None,
            rgb_one_minus_vertex: false, alpha_one_minus_vertex: false,
            tc_mods: Vec::new(), shader_time_offset: 0.0,
            shader_clamp_time: None, authored_glow: false,
        }
    }

    #[test]
    fn material_atomicity_and_original_indices() {
        let draw = FxDraw::Sprite {
            origin: [10.0, 5.0, 1.0], radius: 4.0,
            rotation: 33.0, rgba: [80, 90, 120, 255], shader: "testfx".into(),
        };
        let view = FxView {
            origin: [0.0; 3], axis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            fov_x: 90.0, seconds: 0.1,
        };
        let frame = Arc::new(crate::renderer::FxGpuResidentFrame::default());
        let (surfaces, accepted, _) = tessellate_resident_sprites(
            &[(18, 3, &draw)], Arc::clone(&frame), &view, &mut |_| vec![stage(), stage()],
        );
        assert_eq!(accepted, [18]);
        assert_eq!(surfaces.len(), 2);
        for surface in surfaces {
            let sprite = surface.fx_gpu_sprites.as_ref().unwrap();
            assert_eq!(sprite.resident.as_ref().unwrap().slots.as_slice(), &[3]);
            assert_eq!(sprite.instances.len(), 1, "retain fallback during compilation");
        }
        let bad_stage = FxMaterial { authored_glow: true, ..stage() };
        let (surfaces, accepted, _) = tessellate_resident_sprites(
            &[(18, 3, &draw)], frame, &view, &mut |_| vec![stage(), bad_stage.clone()],
        );
        assert!(accepted.is_empty(), "reject ALL stages when any is unsupported");
        assert!(surfaces.is_empty());
    }

    #[test]
    fn ready_gpu_pipeline_skips_reference_sprite_expansion() {
        let view = FxView {
            origin: [0.0; 3],
            axis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            fov_x: 90.0,
            seconds: 0.0,
        };
        let draw = FxDraw::Sprite {
            origin: [1.0, 2.0, 3.0], radius: 8.0,
            rotation: 22.0, rgba: [255; 4], shader: "testfx".into(),
        };
        let frame = Arc::new(crate::renderer::FxGpuResidentFrame::default());
        let mode = stage().blend.alpha_mode();
        let (cold, accepted, _) = tessellate_resident_sprites(
            &[(0, 12, &draw)], Arc::clone(&frame), &view, &mut |_| vec![stage()],
        );
        assert_eq!(accepted, [0]);
        assert_eq!(cold[0].fx_gpu_sprites.as_ref().unwrap().instances.len(), 1);
        frame.ready_modes.store(1u16 << mode.index(), std::sync::atomic::Ordering::Release);
        let (warm, accepted, warm_ids) = tessellate_resident_sprites(
            &[(0, 12, &draw)], Arc::clone(&frame), &view, &mut |_| vec![stage()],
        );
        assert_eq!(accepted, [0]);
        assert_eq!(warm_ids, [0]);
        let sprites = warm[0].fx_gpu_sprites.as_ref().unwrap();
        assert!(sprites.instances.is_empty(), "GPU-ready path avoids CPU billboard preparation");
        assert_eq!(sprites.resident.as_ref().unwrap().slots.as_slice(), &[12]);
        frame.ready_modes.store(0, std::sync::atomic::Ordering::Release);
        let (rebuild, _, _) = tessellate_resident_sprites(
            &[(0, 12, &draw)], frame, &view, &mut |_| vec![stage()],
        );
        assert_eq!(rebuild[0].fx_gpu_sprites.as_ref().unwrap().instances.len(), 1,
            "pipeline rebuild must restore the fallback");
    }

    #[test]
    fn clock_or_birth_with_no_visible_particle_still_sends_storage_updates() {
        let frame = Arc::new(crate::renderer::FxGpuResidentFrame {
            reset: true, ..Default::default()
        });
        let view = FxView {
            origin: [0.0; 3], axis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            fov_x: 90.0, seconds: 0.0,
        };
        let (surfaces, accepted, _) = tessellate_resident_sprites(&[], frame, &view, &mut |_| vec![]);
        assert!(accepted.is_empty());
        assert_eq!(surfaces.len(), 1);
        assert!(surfaces[0].fx_gpu_sprites.as_ref().unwrap().resident.is_some());
        assert!(surfaces[0].fx_gpu_sprites.as_ref().unwrap().instances.is_empty());
    }
}
