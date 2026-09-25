//! View-dependent FX geometry: OpenJK tr_surface.cpp RB_SurfaceSprite,
//! RB_SurfaceOrientedQuad, RB_SurfaceLine (DoLine) and RB_SurfaceCylinder,
//! built on the CPU against the final render view and batched per shader.

use super::system::{make_normal_vectors, FxDraw};
use crate::{
    camera::Camera,
    materials::TextureData,
    renderer::{DynamicModelAlphaMode, DynamicModelSurface, DynamicModelVertex},
    scene,
};
use std::{collections::HashMap, sync::Arc};

/// Entity number used for FX surfaces (ENTITYNUM_NONE).
const FX_ENTITY_NUM: u16 = 1023;
const NUM_CYLINDER_SEGMENTS: usize = 32;

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FxBlend {
    /// GL_ONE GL_ONE: alpha is irrelevant.
    Add,
    /// GL_SRC_ALPHA GL_ONE.
    AddAlpha,
    /// GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA (and premultiplied variants).
    Alpha,
    /// GL_DST_COLOR GL_ZERO / GL_ZERO GL_SRC_COLOR.
    Modulate,
    /// No blendFunc.
    Opaque,
}

impl FxBlend {
    pub fn from_blend_func(blend: &str) -> Self {
        let words: Vec<_> = blend.split_whitespace().map(str::to_ascii_lowercase).collect();
        match words.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            ["add"] | ["gl_one", "gl_one"] => Self::Add,
            ["gl_src_alpha", "gl_one"] => Self::AddAlpha,
            ["filter"] | ["gl_dst_color", "gl_zero"] | ["gl_zero", "gl_src_color"] => Self::Modulate,
            [] => Self::Opaque,
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

impl FxMaterial {
    /// shaderRGBA through rgbGen/alphaGen to the vertex color the dynamic
    /// pipeline multiplies with the texture.
    fn vertex_color(&self, rgba: [u8; 4]) -> [f32; 4] {
        let rgb = if self.rgb_vertex { [0, 1, 2].map(|i| f32::from(rgba[i]) / 255.0) } else { self.rgb_const };
        let alpha = match self.blend {
            // Additive-one ignores alpha; the pipeline multiplies by it.
            FxBlend::Add | FxBlend::Modulate => 1.0,
            _ if self.alpha_vertex => f32::from(rgba[3]) / 255.0,
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

/// Build per-shader FX surfaces for this view. `material` resolves a shader
/// name (cached by the caller).
pub fn tessellate(
    draws: &[FxDraw],
    view: &FxView,
    material: &mut dyn FnMut(&str) -> FxMaterial,
) -> Vec<DynamicModelSurface> {
    let mut batches: HashMap<String, (FxMaterial, Batch)> = HashMap::new();
    for draw in draws {
        let shader = match draw {
            FxDraw::Sprite { shader, .. }
            | FxDraw::OrientedQuad { shader, .. }
            | FxDraw::Line { shader, .. }
            | FxDraw::Cylinder { shader, .. } => shader,
        };
        let entry = batches
            .entry(shader.to_ascii_lowercase())
            .or_insert_with(|| (material(shader), Batch::default()));
        let (mat, batch) = (&entry.0, &mut entry.1);
        match draw {
            FxDraw::Sprite { origin, radius, rotation, rgba, .. } => {
                let (left, up) = rotated_frame(view.axis[1], view.axis[2], *radius, *rotation);
                batch.quad_stamp(*origin, left, up, mat.vertex_color(*rgba));
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
            FxDraw::Cylinder { start, end, axis, start_radius, end_radius, rgba, .. } => {
                cylinder(batch, *start, *end, *axis, *start_radius, *end_radius, mat.vertex_color(*rgba), view);
            }
        }
    }
    let mut surfaces: Vec<_> = batches
        .into_iter()
        .filter(|(_, (_, batch))| !batch.indices.is_empty())
        .map(|(_, (mat, batch))| DynamicModelSurface {
            entity_num: FX_ENTITY_NUM,
            vertices: Arc::new(batch.vertices),
            indices: Arc::new(batch.indices),
            ghoul2_gpu: None,
            texture: mat.texture.clone(),
            alpha_mode: mat.blend.alpha_mode(),
        })
        .collect();
    // Deterministic submission order across frames.
    surfaces.sort_by_key(|surface| (surface.alpha_mode as u8, surface.vertices.len()));
    surfaces
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
        assert_eq!(FxBlend::from_blend_func(""), FxBlend::Opaque);
    }

    #[test]
    fn sprite_faces_view_with_radius_half_extent_and_add_ignores_alpha_byte() {
        let draws = [FxDraw::Sprite { origin: [100.0, 0.0, 0.0], radius: 2.0, rotation: 0.0, rgba: [255, 128, 0, 0], shader: "a".into() }];
        let surfaces = tessellate(&draws, &view(), &mut |_| white(FxBlend::Add));
        let surface = &surfaces[0];
        assert_eq!(surface.alpha_mode, DynamicModelAlphaMode::AdditiveOne);
        let jka: Vec<_> = surface.vertices.iter().map(|v| scene::jka_position(v.position)).collect();
        assert_eq!(jka[0], [100.0, 2.0, 2.0], "origin + left + up");
        assert_eq!(jka[2], [100.0, -2.0, -2.0]);
        assert_eq!(surface.vertices[0].color, [1.0, 128.0 / 255.0, 0.0, 1.0]);
        assert_eq!(surface.indices.len(), 12, "two triangles, both windings");
    }

    #[test]
    fn line_width_spans_perpendicular_to_view_and_segment() {
        let draws = [FxDraw::Line { start: [100.0, 0.0, 0.0], end: [100.0, 50.0, 0.0], width: 1.5, rgba: [255, 255, 255, 128], shader: "b".into() }];
        let surfaces = tessellate(&draws, &view(), &mut |_| white(FxBlend::AddAlpha));
        let jka: Vec<_> = surfaces[0].vertices.iter().map(|v| scene::jka_position(v.position)).collect();
        // (start - view) x (end - view) points along +Z here.
        assert!((jka[0][2] - 1.5).abs() < 1e-4 && (jka[1][2] + 1.5).abs() < 1e-4);
        assert!((surfaces[0].vertices[0].color[3] - 128.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn batches_by_shader_and_cylinder_rings_use_both_radii() {
        let draws = [
            FxDraw::Sprite { origin: [10.0, 0.0, 0.0], radius: 1.0, rotation: 30.0, rgba: [255; 4], shader: "x".into() },
            FxDraw::Sprite { origin: [20.0, 0.0, 0.0], radius: 1.0, rotation: 0.0, rgba: [255; 4], shader: "X".into() },
            FxDraw::Cylinder { start: [0.0; 3], end: [0.0, 0.0, 10.0], axis: [0.0, 0.0, 1.0], start_radius: 1.0, end_radius: 3.0, rgba: [255; 4], shader: "c".into() },
        ];
        let surfaces = tessellate(&draws, &view(), &mut |_| white(FxBlend::Alpha));
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
