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

/// The subset of a shader's primary stage an FX sprite needs.
#[derive(Clone, Debug)]
pub struct FxMaterial {
    pub texture: Option<Arc<TextureData>>,
    pub blend: FxBlend,
    /// rgbGen vertex/exactVertex (FX shaders normally use vertex color).
    pub rgb_vertex: bool,
    pub alpha_vertex: bool,
    pub rgb_const: [f32; 3],
    pub alpha_const: f32,
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
    /// shaderRGBA through rgbGen/alphaGen to the vertex color the dynamic
    /// pipeline multiplies with the texture.
    fn vertex_color(&self, rgba: [u8; 4]) -> [f32; 4] {
        self.float_vertex_color([
            f32::from(rgba[0]) / 255.0,
            f32::from(rgba[1]) / 255.0,
            f32::from(rgba[2]) / 255.0,
            f32::from(rgba[3]) / 255.0,
        ])
    }

    /// Same shader stage color semantics for CGame's float R_SetColor path.
    pub(crate) fn float_vertex_color(&self, rgba: [f32; 4]) -> [f32; 4] {
        let rgb = if self.rgb_vertex { [rgba[0], rgba[1], rgba[2]] } else { self.rgb_const };
        let alpha = match self.blend {
            // GL_ONE-style stages ignore source alpha in their blend equation.
            FxBlend::Add
            | FxBlend::Modulate
            | FxBlend::DstColorAdd
            | FxBlend::Modulate2x
            | FxBlend::Darken => 1.0,
            _ if self.alpha_vertex => rgba[3],
            _ => self.alpha_const,
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
            | FxDraw::Cylinder { shader, .. } => shader,
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
            texture: mat.texture.clone(),
            alpha_mode: mat.blend.alpha_mode(),
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
            | FxDraw::Cylinder { shader, .. } => shader,
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
                texture: mat.texture.clone(),
                alpha_mode: mat.blend.alpha_mode(),
            })
        })
        .collect());

    surfaces.extend(lit_surfaces);
    surfaces.sort_by_key(|surface| (surface.alpha_mode as u8, surface.vertices.len()));
    surfaces
}

/// Keep JKA/OpenJK FX simulation semantics on the CPU, but submit RT_SPRITE
/// (`Particle` in .efx files) as compact WGPU instances. The final billboard
/// basis is still evaluated against the final render view on the CPU so this
/// path changes submission cost, not authored motion/lifetime/orientation.
/// Non-sprite FX primitives keep the reference CPU tessellation path.
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
            | FxDraw::Cylinder { shader, .. } => shader,
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
            if let FxDraw::Sprite { origin, radius, rotation, rgba, .. } = draw {
                let (left, up) = rotated_frame(view.axis[1], view.axis[2], *radius, *rotation);
                let origin = scene::render_position(*origin);
                let left = scene::render_position(left);
                let up = scene::render_position(up);
                sprite_batches
                    .entry((shader_key.clone(), stage_index))
                    .or_insert_with(|| (mat.clone(), Vec::new()))
                    .1
                    .push(FxGpuSpriteInstance {
                        origin: [origin[0], origin[1], origin[2], 0.0],
                        left: [left[0], left[1], left[2], 0.0],
                        up: [up[0], up[1], up[2], 0.0],
                        color: mat.vertex_color(*rgba),
                    });
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
            texture: mat.texture.clone(),
            alpha_mode: mat.blend.alpha_mode(),
        })
        .collect();

    surfaces.extend(
        sprite_batches
            .into_iter()
            .filter(|(_, (_, instances))| !instances.is_empty())
            .map(|(_, (mat, instances))| DynamicModelSurface {
                entity_num: FX_ENTITY_NUM,
                wireframe_class: DynamicWireframeClass::Effect,
                raster_visible: true,
                vertices: Arc::new(Vec::new()),
                indices: Arc::new(Vec::new()),
                lighting_origin: None,
                rt_rigid: None,
                rt_skinned_key: None,
                ghoul2_gpu: None,
                fx_gpu_sprites: Some(FxGpuSprites { instances: Arc::new(instances), blob_shadow: false }),
                texture: mat.texture.clone(),
                alpha_mode: mat.blend.alpha_mode(),
            }),
    );

    surfaces.extend(lit_surfaces);
    surfaces.sort_by_key(|surface| (surface.alpha_mode as u8, surface.vertex_count()));
    surfaces
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
    })
}

fn append_batch(dst: &mut Batch, mut src: Batch) {
    let base = dst.vertices.len() as u32;
    dst.vertices.append(&mut src.vertices);
    dst.indices
        .extend(src.indices.into_iter().map(|index| index.saturating_add(base)));
}

fn append_draw(draw: &FxDraw, view: &FxView, mat: &FxMaterial, batch: &mut Batch) {
    match draw {
        FxDraw::Sprite { origin, radius, rotation, rgba, .. } => {
            let (left, up) = rotated_frame(view.axis[1], view.axis[2], *radius, *rotation);
            batch.quad_stamp(*origin, left, up, mat.vertex_color(*rgba));
        }
        FxDraw::SaberGlow { origin, direction, length, radius, hilt_radius, rgba, .. } => {
            // TaystJK/OpenJK RB_SurfaceSaberGlow. The renderer receives one
            // RT_SABER_GLOW entity, then walks billboards from blade tip back
            // toward the hilt. Radius grows by exactly 0.017 per blob and the
            // step is exactly 65% of the current radius; do not clamp it here.
            let color = mat.vertex_color(*rgba);
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
        FxDraw::OrientedQuad { origin, axis, radius, rotation, rgba, .. } => {
            let (left, up) = rotated_frame(axis[1], axis[2], *radius, *rotation);
            batch.quad_stamp(*origin, left, up, mat.vertex_color(*rgba));
        }
        FxDraw::Line { start, end, width, rgba, .. } => {
            // RB_SurfaceLine: right = normalize((start-view) x (end-view)).
            let right = normalize(cross(sub(*start, view.origin), sub(*end, view.origin)));
            let color = mat.vertex_color(*rgba);
            let v0 = batch.vertex(add(*start, scale(right, *width)), [0.0, 0.0], color);
            let v1 = batch.vertex(sub(*start, scale(right, *width)), [1.0, 0.0], color);
            let v2 = batch.vertex(add(*end, scale(right, *width)), [0.0, 1.0], color);
            let v3 = batch.vertex(sub(*end, scale(right, *width)), [1.0, 1.0], color);
            batch.triangle(v0, v1, v2);
            batch.triangle(v2, v1, v3);
        }
        FxDraw::Quad { positions, uvs, rgba, .. } => {
            let color = mat.vertex_color(*rgba);
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
                batch.vertex(*position, *uv, mat.vertex_color(*color));
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
            cylinder(batch, *start, *end, *axis, *start_radius, *end_radius, mat.vertex_color(*rgba), view);
        }
    }
}

/// RB_SurfaceSprite/OrientedQuad rotation of the left/up frame.
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

    fn white(blend: FxBlend) -> FxMaterial {
        FxMaterial { texture: None, blend, rgb_vertex: true, alpha_vertex: true, rgb_const: [1.0; 3], alpha_const: 1.0 }
    }

    fn view() -> FxView {
        FxView { origin: [0.0; 3], axis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], fov_x: 90.0 }
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
