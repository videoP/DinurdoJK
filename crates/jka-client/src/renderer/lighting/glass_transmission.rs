//! Conservative runtime glass classification and linear RGB throughput.
//!
//! q3map2 `lightfilter` is positive authored evidence, not a synonym for
//! `trans` or for alpha-shadow foliage. Unmarked glass is accepted only when
//! both the texture name and the draw semantics corroborate it.

use crate::material_flags::*;
use crate::renderer::{BlendMode, DrawBatch, DrawClass};
use std::sync::OnceLock;
use std::collections::HashMap;

fn user_glass_override(name: &str) -> Option<bool> {
    // Read exactly once, on map load. No filesystem calls on the render hot path.
    static OVERRIDES: OnceLock<HashMap<String, bool>> = OnceLock::new();
    let overrides = OVERRIDES.get_or_init(|| {
        let mut result = HashMap::new();
        let Ok(text) = std::fs::read_to_string("tinted_glass_overrides.txt") else { return result; };
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((name, value)) = line.split_once('=') else { continue; };
            let name = name.trim().to_ascii_lowercase();
            let enabled = match value.trim().to_ascii_lowercase().as_str() {
                "1" | "yes" | "true" | "on" => Some(true),
                "0" | "no" | "false" | "off" => Some(false),
                _ => None,
            };
            if let Some(enabled) = enabled { result.insert(name, enabled); }
        }
        result
    });
    overrides.get(&name.to_ascii_lowercase()).copied()
}


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::renderer) enum GlassSource {
    Authored,
    ExplicitOverride,
    Heuristic,
}

/// Optional per-material override: Some(true) forces transmission,
/// Some(false) vetoes heuristic detection. Authored `lightfilter` wins first;
/// overrides are a second layer for materials without that metadata.
pub(in crate::renderer) fn classify_glass(
    batch: &DrawBatch,
    texture_name: Option<&str>,
    override_transmission: Option<bool>,
    allow_heuristics: bool,
) -> Option<GlassSource> {
    if batch.water || batch.pipeline.class == DrawClass::Sky {
        return None;
    }
    // Authored transmission is stronger evidence than generic alpha-shadow
    // flags that a multi-stage material might happen to carry as well.
    if batch.light_filter {
        return Some(GlassSource::Authored);
    }
    if batch.alpha_shadow {
        return None;
    }
    if let Some(force) = override_transmission.or_else(|| texture_name.and_then(user_glass_override)) {
        return force.then_some(GlassSource::ExplicitOverride);
    }
    if !allow_heuristics || user_glass_override("auto") == Some(false)
        || batch.pipeline.class != DrawClass::Transparent
        || batch.pipeline.blend == BlendMode::Opaque || batch.pipeline.depth_write
    {
        return None;
    }
    let name = texture_name?.to_ascii_lowercase();
    // Never infer from generic alpha blending, portals, water or vegetation.
    if name.contains("water") || name.contains("portal") || name.contains("mirror")
        || name.contains("leaf") || name.contains("foliage") || name.contains("grate")
        || name.contains("screen") || name.contains("hologram")
    {
        return None;
    }
    (name.contains("glass") || name.contains("windowpane") || name.contains("stained_window"))
        .then_some(GlassSource::Heuristic)
}

/// Linear, energy-non-increasing per-layer RGB throughput. The alpha channel
/// is the authored fractional opacity: 0 = no absorption, 1 = no throughput.
/// `tint` must already be converted from sRGB to linear light.
pub(in crate::renderer) fn glass_throughput_linear(
    tint: [f32; 3],
    opacity: f32,
) -> [f32; 3] {
    let transparency = 1.0 - opacity.clamp(0.0, 1.0);
    tint.map(|v| v.clamp(0.0, 1.0) * transparency)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stacked_panes_multiply_without_brightening() {
        let red = glass_throughput_linear([1.0, 0.2, 0.1], 0.25);
        let blue = glass_throughput_linear([0.1, 0.2, 1.0], 0.25);
        let stacked: Vec<_> = red.into_iter().zip(blue).map(|(a, b)| a * b).collect();
        assert!(stacked[0] < red[0]);
        assert!(stacked[2] < blue[2]);
        assert_eq!(glass_throughput_linear([1.0; 3], 1.0), [0.0; 3]);
        assert_eq!(glass_throughput_linear([1.0; 3], 0.0), [1.0; 3]);
    }
}


// Bounded receiver-side raster transmission: a per-light glass-caster table.
// Shader ray segments stop at the receiver, while the existing opaque depth cube
// rejects occluded receivers. A slot with no glass has a zero count and no loops.
use crate::renderer::{RtAlphaMaskSource, Vec3};
use bytemuck::{Pod, Zeroable};

pub(in crate::renderer) const RASTER_GLASS_PER_LIGHT: usize = 64;
// Reserve independent caster slots for the directional sun. Raster local lights
// keep their existing 8 slots; no bind-group ABI changes are required.
pub(in crate::renderer) const RASTER_SUN_GLASS_SLOTS: usize = 8;

#[repr(C)]
#[derive(Clone, Copy, Default, Pod, Zeroable)]
pub(in crate::renderer) struct RasterGlassTriangle {
    pub(in crate::renderer) a: [f32; 4],
    pub(in crate::renderer) b: [f32; 4],
    pub(in crate::renderer) c: [f32; 4],
    pub(in crate::renderer) color_a: [f32; 4],
    pub(in crate::renderer) color_b: [f32; 4],
    pub(in crate::renderer) color_c: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct RasterGlassSlot {
    pub(in crate::renderer) header: [u32; 4],
    pub(in crate::renderer) triangles: [RasterGlassTriangle; RASTER_GLASS_PER_LIGHT],
}

impl Default for RasterGlassSlot {
    fn default() -> Self {
        Self { header: [0; 4], triangles: [RasterGlassTriangle::default(); RASTER_GLASS_PER_LIGHT] }
    }
}

fn sample_texture(source: &RtAlphaMaskSource, uv: [f32; 2]) -> [f32; 4] {
    let tex = &source.material.texture;
    let (width, height) = (tex.width.max(1), tex.height.max(1));
    let wrap = |u: f32| if tex.clamp { u.clamp(0.0, 0.999999) } else { u.rem_euclid(1.0) };
    let x = (wrap(uv[0]) * width as f32) as usize;
    let y = (wrap(uv[1]) * height as f32) as usize;
    let index = y * width as usize + x;
    let word = if tex.color {
        tex.pixels.get(index).copied().unwrap_or(0xffffffff)
    } else {
        // Alpha-mask packing uses four texels per word.
        let packed = tex.pixels.get(index / 4).copied().unwrap_or(u32::MAX);
        let alpha = (packed >> ((index % 4) * 8)) & 0xff;
        0x00ffffff | (alpha << 24)
    };
    let bytes = word.to_le_bytes();
    bytes.map(|v| v as f32 / 255.0)
}

fn sampled_throughput(source: &RtAlphaMaskSource, vertex: &crate::renderer::RtAlphaVertexGpu) -> [f32; 4] {
    let material = &source.material.uniform;
    let mut uv = [vertex.position_u[3], vertex.normal_v[3]];
    match material.header[0] {
        1 => uv = [vertex.lightmap_alpha[0], vertex.lightmap_alpha[1]],
        2 => {
            let position = Vec3::from_slice(&vertex.position_u[..3]);
            uv = [position.dot(Vec3::from_slice(&material.vector_s[..3])),
                  position.dot(Vec3::from_slice(&material.vector_t[..3]))];
        }
        _ => {}
    }
    let sampled = sample_texture(source, uv);
    let vertex_alpha = if material.header[2] & MATERIAL_FLAG_ALPHA_VERTEX != 0 {
        vertex.lightmap_alpha[2]
    } else if material.header[2] & MATERIAL_FLAG_ALPHA_ONE_MINUS_VERTEX != 0 {
        1.0 - vertex.lightmap_alpha[2]
    } else { 1.0 };
    let alpha = (sampled[3] * material.color[3] * vertex_alpha).clamp(0.0, 1.0);
    let opacity = if alpha >= 0.98 || source.material.was_opaque { 0.25 } else { alpha };
    let linear = [sampled[0], sampled[1], sampled[2]]
        .map(|s| if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) });
    let rgb = glass_throughput_linear([
        linear[0] * material.color[0],
        linear[1] * material.color[1],
        linear[2] * material.color[2],
    ], opacity);
    [rgb[0], rgb[1], rgb[2], 1.0]
}

pub(in crate::renderer) fn raster_glass_triangles(sources: &[RtAlphaMaskSource]) -> Vec<RasterGlassTriangle> {
    let mut result = Vec::new();
    for source in sources.iter().filter(|s| s.material.glass) {
        for triangle in source.indices.chunks_exact(3) {
            let Some(a) = source.vertices.get(triangle[0] as usize) else { continue; };
            let Some(b) = source.vertices.get(triangle[1] as usize) else { continue; };
            let Some(c) = source.vertices.get(triangle[2] as usize) else { continue; };
            let position = |v: &crate::renderer::RtAlphaVertexGpu| [v.position_u[0], v.position_u[1], v.position_u[2], 0.0];
            let ab = Vec3::from_slice(&a.position_u[..3]);
            let bb = Vec3::from_slice(&b.position_u[..3]);
            let cb = Vec3::from_slice(&c.position_u[..3]);
            if (bb-ab).cross(cb-ab).length_squared() <= 1e-8 { continue; }
            result.push(RasterGlassTriangle {
                a: position(a), b: position(b), c: position(c),
                color_a: sampled_throughput(source, a),
                color_b: sampled_throughput(source, b),
                color_c: sampled_throughput(source, c),
            });
        }
    }
    result
}

pub(in crate::renderer) fn triangle_intersects_light(tri: &RasterGlassTriangle, origin: [f32; 3], radius: f32) -> bool {
    let r = radius * radius;
    let mut dist2 = 0.0_f32;
    for i in 0..3 {
        let lo = tri.a[i].min(tri.b[i]).min(tri.c[i]);
        let hi = tri.a[i].max(tri.b[i]).max(tri.c[i]);
        let nearest = origin[i].clamp(lo, hi);
        dist2 += (origin[i] - nearest) * (origin[i] - nearest);
    }
    dist2 <= r
}

pub(in crate::renderer) fn transformed_triangle(mut tri: RasterGlassTriangle, pose: crate::renderer::InlineModelInstance) -> RasterGlassTriangle {
    for point in [&mut tri.a, &mut tri.b, &mut tri.c] {
        let world = pose.transform_point([point[0], point[1], point[2]]);
        point[..3].copy_from_slice(&world);
    }
    tri
}

#[cfg(test)]
mod raster_tests {
    use super::*;
    #[test]
    fn cached_glass_respects_light_radius_and_never_emits_partial_layers() {
        let mut tri = RasterGlassTriangle::default();
        tri.a = [0.0, 0.0, 2.0, 0.0];
        tri.b = [2.0, 0.0, 2.0, 0.0];
        tri.c = [0.0, 2.0, 2.0, 0.0];
        let (near, count) = pack_raster_glass_slot([0.0, 0.0, 0.0], 4.0, &[tri], &[]);
        assert_eq!((count, near.header[0]), (1, 1));
        let (far, count) = pack_raster_glass_slot([20.0, 20.0, 20.0], 4.0, &[tri], &[]);
        assert_eq!((count, far.header[0]), (0, 0));
        let saturated = vec![tri; RASTER_GLASS_PER_LIGHT + 1];
        let (over, count) = pack_raster_glass_slot([0.0, 0.0, 0.0], 4.0, &saturated, &[]);
        assert_eq!(count, RASTER_GLASS_PER_LIGHT + 1);
        assert_eq!(over.header[0], 0);
    }
    #[test]
    fn bounded_slots_are_zeroed_and_color_linear() {
        assert_eq!(RasterGlassSlot::default().header[0], 0);
        assert!(std::mem::size_of::<RasterGlassSlot>() < 32 * 1024);
        assert!(glass_throughput_linear([0.5; 3], 0.25)[0] < 0.5);
    }
}


pub(in crate::renderer) fn pack_raster_glass_slot(
    light_origin: [f32; 3], radius: f32,
    static_triangles: &[RasterGlassTriangle], dynamic_triangles: &[RasterGlassTriangle],
) -> (RasterGlassSlot, usize) {
    let mut slot = RasterGlassSlot::default();
    let mut total = 0usize;
    for triangle in static_triangles.iter().chain(dynamic_triangles) {
        if triangle_intersects_light(triangle, light_origin, radius) {
            if total < RASTER_GLASS_PER_LIGHT { slot.triangles[total] = *triangle; }
            total += 1;
        }
    }
    // An incomplete set would incorrectly brighten receivers. Disable this
    // light's tint rather than passing only some of its eligible panes.
    if total <= RASTER_GLASS_PER_LIGHT { slot.header[0] = total as u32; }
    (slot, total)
}

/// Fill all directional-sun slots atomically. No partial prefix: if the glass
/// surface exceeds capacity, sunlight tint is disabled rather than treating
/// unrepresented panes as clear. The opaque sun cascade remains authoritative.
fn pack_raster_sun_glass(
    static_triangles: &[RasterGlassTriangle], dynamic_triangles: &[RasterGlassTriangle],
) -> Vec<RasterGlassSlot> {
    let total = static_triangles.len() + dynamic_triangles.len();
    let capacity = RASTER_SUN_GLASS_SLOTS * RASTER_GLASS_PER_LIGHT;
    let mut slots = vec![RasterGlassSlot::default(); RASTER_SUN_GLASS_SLOTS];
    if total <= capacity {
        for (index, triangle) in static_triangles.iter().chain(dynamic_triangles).enumerate() {
            let slot = index / RASTER_GLASS_PER_LIGHT;
            slots[slot].triangles[index % RASTER_GLASS_PER_LIGHT] = *triangle;
            slots[slot].header[0] += 1;
        }
        // Each 64-triangle slot stores a conservative bounding sphere in the
        // unused w components of its first triangle. Receiver shader quickly
        // rejects a sun ray outside that sphere before looping the triangles.
        // RGB uses only .xyz, so all existing color math stays unchanged.
        for slot in &mut slots {
            if slot.header[0] == 0 { continue; }
            let mut lo = [f32::INFINITY; 3];
            let mut hi = [f32::NEG_INFINITY; 3];
            for tri in &slot.triangles[..slot.header[0] as usize] {
                for v in [&tri.a, &tri.b, &tri.c] {
                    for axis in 0..3 {
                        lo[axis] = lo[axis].min(v[axis]);
                        hi[axis] = hi[axis].max(v[axis]);
                    }
                }
            }
            let center = std::array::from_fn::<f32, 3, _>(|axis| (lo[axis] + hi[axis]) * 0.5);
            let radius = ((hi[0] - lo[0]).powi(2)
                + (hi[1] - lo[1]).powi(2)
                + (hi[2] - lo[2]).powi(2)).sqrt() * 0.5 + 0.1;
            slot.triangles[0].a[3] = center[0];
            slot.triangles[0].b[3] = center[1];
            slot.triangles[0].c[3] = center[2];
            slot.triangles[0].color_a[3] = radius;
        }
    }
    slots
}

pub(in crate::renderer) fn upload_raster_sun_glass(
    queue: &wgpu::Queue, buffer: &wgpu::Buffer,
    static_triangles: &[RasterGlassTriangle], dynamic_triangles: &[RasterGlassTriangle],
) -> usize {
    let slots = pack_raster_sun_glass(static_triangles, dynamic_triangles);
    queue.write_buffer(buffer,
        (crate::renderer::LOCAL_SHADOW_CACHE_SLOTS * std::mem::size_of::<RasterGlassSlot>()) as u64,
        bytemuck::cast_slice(&slots));
    static_triangles.len() + dynamic_triangles.len()
}

#[cfg(test)]
mod solar_tests {
    use super::*;
    #[test]
    fn sunlight_capacity_and_layout() {
        let cap = RASTER_SUN_GLASS_SLOTS * RASTER_GLASS_PER_LIGHT;
        assert!(cap >= 256);
        assert_eq!(std::mem::size_of::<RasterGlassSlot>(), 16 + 64 * 96);
        let mut tri = RasterGlassTriangle::default();
        tri.a = [-2.0, -2.0, 5.0, 0.0];
        tri.b = [2.0, -2.0, 5.0, 0.0];
        tri.c = [0.0, 2.0, 5.0, 0.0];
        tri.color_a = [0.7, 0.1, 0.1, 1.0];
        let slots = pack_raster_sun_glass(&[tri], &[]);
        assert_eq!(slots[0].header[0], 1);
        assert_eq!(slots[1].header[0], 0);
        assert_eq!(slots[0].triangles[0].a[3], 0.0);
        assert_eq!(slots[0].triangles[0].c[3], 5.0);
        assert!(slots[0].triangles[0].color_a[3] > 2.0);
        assert_eq!(slots[0].triangles[0].color_a[0], 0.7);
        let overflow = pack_raster_sun_glass(&vec![tri; cap + 1], &[]);
        assert!(overflow.iter().all(|slot| slot.header[0] == 0));
        let stacked = pack_raster_sun_glass(&[tri; 65], &[]);
        assert_eq!(stacked[0].header[0], 64);
        assert_eq!(stacked[1].header[0], 1);
    }
}
