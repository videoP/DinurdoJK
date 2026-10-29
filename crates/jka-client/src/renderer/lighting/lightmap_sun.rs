//! Camera-independent static sunlight in lightmap UV space. The existing AO
//! rasterizer supplies receivers; one cancellable worker traces static visibility.
//! The shared lightmap compositor owns the final subtraction/tone/sun result.
use crate::renderer::{
    Arc, DrawBatch, DrawClass, GpuImage, GpuVertex, Renderer, StaticAoBvh, StaticAoRenderTriangle,
    SunVisibilityMode, TextureData, Vec3, scene,
};
use rayon::prelude::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
};

#[derive(Clone)]
pub(in crate::renderer) struct Geometry {
    vertices: Arc<Vec<GpuVertex>>,
    batches: Vec<DrawBatch>,
    textures: Arc<Vec<TextureData>>,
    receivers: Arc<Vec<crate::renderer::StaticAoLightmapTriangle>>,
    model_materials: Vec<bool>,
}

impl Geometry {
    pub(in crate::renderer) fn new(
        vertices: Arc<Vec<GpuVertex>>,
        batches: Vec<DrawBatch>,
        textures: Arc<Vec<TextureData>>,
        materials: &scene::MaterialDebugInfo,
    ) -> Self {
        let receivers = Arc::new(crate::renderer::collect_static_ao_lightmap_triangles(
            &vertices, &batches,
        ));
        let model_materials = batches
            .iter()
            .map(|b| {
                materials
                    .entries
                    .get(b.material_debug_index as usize)
                    .is_some_and(|m| m.name.starts_with("models/"))
            })
            .collect();
        Self {
            vertices,
            batches,
            textures,
            receivers,
            model_materials,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Key {
    direction: [f32; 3],
    visibility: SunVisibilityMode,
    models: bool,
    tinted: bool,
    sizes: Vec<[u32; 2]>,
    bake_sun: bool,
    local_sources: u32,
    local_sizes: Vec<[u32; 2]>,
    locals: Vec<(usize, scene::DynamicLight)>,
}

struct BakeOutput {
    sun: Vec<Vec<f32>>,
    local: Vec<Vec<f32>>,
}

struct Pending {
    key: Key,
    rx: mpsc::Receiver<Result<BakeOutput, String>>,
    cancel: Arc<AtomicBool>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub(in crate::renderer) struct StaticSun {
    pub geometry: Option<Geometry>,
    pending: Option<Pending>,
    key: Option<Key>,
    pub flux: Vec<GpuImage>,
    pub local_flux: Vec<GpuImage>,
    pub local_lights: Vec<(usize, scene::DynamicLight)>,
    pub generation: u64,
    failed: Option<Key>,
    source_sizes: Vec<[u32; 2]>,
}

/// Upsample sunlight independently of the authored indirect light. Half-float
/// colour plus the R32F incident-flux image share a bounded 128 MiB budget.
pub(super) fn output_sizes(base: &[[u32; 2]], limit: u32) -> Vec<[u32; 2]> {
    if base.iter().any(|s| s[0] > limit || s[1] > limit)
        || base
            .iter()
            .map(|s| u64::from(s[0]) * u64::from(s[1]) * 16)
            .sum::<u64>()
            > 128 * 1024 * 1024
    {
        return Vec::new();
    }
    let mut scale = 4u32;
    while scale > 1
        && (base
            .iter()
            .any(|s| s[0].saturating_mul(scale) > limit || s[1].saturating_mul(scale) > limit)
            || base
                .iter()
                .map(|s| u64::from(s[0]) * u64::from(s[1]) * u64::from(scale * scale) * 16)
                .sum::<u64>()
                > 128 * 1024 * 1024)
    {
        scale /= 2;
    }
    base.iter().map(|s| [s[0] * scale, s[1] * scale]).collect()
}

pub(in crate::renderer) fn moving_static_material(
    batch: &DrawBatch,
    visibility: SunVisibilityMode,
) -> bool {
    (batch.alpha_cutoff != 0.0
        || (visibility == SunVisibilityMode::Filtered
            && batch.pipeline.class == DrawClass::Transparent))
        && !fixed_uv(batch)
}

fn fixed_uv(batch: &DrawBatch) -> bool {
    batch.animation.is_none()
        && !matches!(batch.alpha_gen, jka_assets::shader::AlphaGen::Wave(_))
        && matches!(
            batch.tc_gen,
            jka_assets::shader::TcGen::Base
                | jka_assets::shader::TcGen::Lightmap
                | jka_assets::shader::TcGen::Vector(..)
        )
        && batch.tc_mods.iter().all(|m| {
            matches!(
                m,
                jka_assets::shader::TcMod::Scale(..) | jka_assets::shader::TcMod::Transform(..)
            )
        })
}

fn alpha_at(
    batch: &DrawBatch,
    vertices: &[GpuVertex; 3],
    bary: [f32; 3],
    image: &TextureData,
) -> f32 {
    let mut uv = [0.0; 2];
    let p = (0..3).fold(Vec3::ZERO, |p, i| {
        p + Vec3::from_array(vertices[i].position) * bary[i]
    });
    for axis in 0..2 {
        uv[axis] = (0..3)
            .map(|i| {
                (if matches!(batch.tc_gen, jka_assets::shader::TcGen::Lightmap) {
                    vertices[i].lightmap_uv[axis]
                } else {
                    vertices[i].uv[axis]
                }) * bary[i]
            })
            .sum();
    }
    if let jka_assets::shader::TcGen::Vector(s, t) = batch.tc_gen {
        uv = [p.dot(Vec3::from_array(s)), p.dot(Vec3::from_array(t))];
    }
    for m in batch.tc_mods.iter().take(4) {
        match *m {
            jka_assets::shader::TcMod::Scale(s, t) => {
                uv[0] *= s;
                uv[1] *= t;
            }
            jka_assets::shader::TcMod::Transform(v) => {
                uv = [
                    uv[0] * v[0] + uv[1] * v[2] + v[4],
                    uv[0] * v[1] + uv[1] * v[3] + v[5],
                ];
            }
            _ => {}
        }
    }
    let coord = |v: f32, n: u32| -> usize {
        ((if image.clamp {
            v.clamp(0.0, 1.0 - f32::EPSILON)
        } else {
            v - v.floor()
        }) * n as f32) as usize
    };
    let pixel = (coord(uv[1], image.height) * image.width as usize + coord(uv[0], image.width)) * 4;
    let vertex_alpha = if matches!(batch.alpha_gen, jka_assets::shader::AlphaGen::Vertex) {
        (0..3).map(|i| vertices[i].color[3] * bary[i]).sum()
    } else {
        1.0
    };
    image.rgba.get(pixel + 3).copied().unwrap_or(255) as f32 / 255.0 * batch.color[3] * vertex_alpha
}

fn fill_chart_border(values: &mut [f32], size: [u32; 2]) {
    let original = values.to_vec();
    for (i, value) in values.iter_mut().enumerate().filter(|(_, v)| **v < 0.0) {
        let x = i % size[0] as usize;
        let y = i / size[0] as usize;
        let mut sum = 0.0;
        let mut count = 0;
        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
            if nx >= 0 && ny >= 0 && nx < size[0] as i32 && ny < size[1] as i32 {
                let v = original[ny as usize * size[0] as usize + nx as usize];
                if v >= 0.0 {
                    sum += v;
                    count += 1;
                }
            }
        }
        *value = if count > 0 { sum / count as f32 } else { 0.0 };
    }
}

fn trace(geometry: &Geometry, key: &Key, cancel: &AtomicBool) -> Result<BakeOutput, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    let mut triangles = Vec::new();
    let mut filters = Vec::new();
    let mut sky = Vec::new();
    for (index, batch) in geometry.batches.iter().enumerate() {
        if !key.models && geometry.model_materials[index] {
            continue;
        }
        if batch.pipeline.depth_equal {
            continue;
        }
        let is_sky = batch.pipeline.class == DrawClass::Sky
            && !matches!(batch.tc_gen, jka_assets::shader::TcGen::SkyCloud(_));
        let masked = batch.alpha_cutoff != 0.0;
        let translucent = key.visibility == SunVisibilityMode::Filtered
            && batch.pipeline.class == DrawClass::Transparent;
        if !is_sky && batch.pipeline.class != DrawClass::Opaque && !masked && !translucent {
            continue;
        }
        if (masked || translucent) && !fixed_uv(batch) {
            continue;
        }
        if key.tinted
            && crate::renderer::lighting::glass_transmission::classify_glass(
                batch,
                batch
                    .texture
                    .and_then(|t| geometry.textures.get(t))
                    .map(|i| i.label.as_str()),
                None,
                true,
            )
            .is_some()
        {
            return Err("a glass caster requires live coloured transmission".into());
        }
        for triangle in geometry.vertices
            [batch.vertices.start as usize..batch.vertices.end as usize]
            .chunks_exact(3)
        {
            let v = [triangle[0], triangle[1], triangle[2]];
            let triangle = StaticAoRenderTriangle {
                positions: v.map(|v| v.position),
            };
            if is_sky {
                sky.push(triangle);
            } else {
                triangles.push(triangle);
                filters.push((masked || translucent).then_some((index, v, translucent)));
            }
        }
    }
    let solid = (!triangles.is_empty())
        .then(|| StaticAoBvh::build(Arc::new(triangles)))
        .transpose()?;
    let sky = (!sky.is_empty())
        .then(|| StaticAoBvh::build(Arc::new(sky)))
        .transpose()?;
    let visibility = |origin: Vec3, toward: Vec3, distance: f32| {
        let mut transmission = 1.0;
        let blocked = solid
            .as_ref()
            .and_then(|bvh| {
                bvh.trace_filtered(origin, toward, (distance - 0.1).max(0.0), |index, bary| {
                    let Some((batch, v, translucent)) = &filters[index] else {
                        return true;
                    };
                    let batch = &geometry.batches[*batch];
                    let alpha = batch
                        .texture
                        .and_then(|t| geometry.textures.get(t))
                        .map_or(1.0, |image| alpha_at(batch, v, bary, image));
                    if *translucent {
                        transmission *= 1.0 - alpha.clamp(0.0, 1.0);
                        transmission <= 0.001
                    } else if batch.alpha_cutoff > 0.0 {
                        alpha >= batch.alpha_cutoff
                    } else {
                        alpha < -batch.alpha_cutoff
                    }
                })
            })
            .is_some();
        if blocked { 0.0 } else { transmission }
    };
    let samples = crate::renderer::static_ao::bvh::rasterize_lightmap_texels(
        &geometry.receivers,
        &key.sizes,
        1,
    );
    let toward = -Vec3::from_array(key.direction).normalize_or_zero();
    let pages = samples
        .par_iter()
        .zip(&key.sizes)
        .map(|(samples, size)| {
            let mut flux: Vec<f32> = samples
                .par_iter()
                .map(|sample| {
                    if cancel.load(Ordering::Relaxed) {
                        return -1.0;
                    }
                    let Some(sample) = sample else {
                        return -1.0;
                    };
                    if !key.bake_sun {
                        return 0.0;
                    }
                    let normal = Vec3::from_array(sample.normal);
                    let facing = normal.dot(toward).max(0.0);
                    if facing == 0.0 {
                        return 0.0;
                    }
                    let origin = Vec3::from_array(sample.position) + normal * 0.5;
                    let distance = if key.visibility == SunVisibilityMode::Legacy {
                        131072.0
                    } else {
                        let Some(d) = sky.as_ref().and_then(|s| s.trace(origin, toward, 131072.0))
                        else {
                            return 0.0;
                        };
                        d
                    };
                    facing * visibility(origin, toward, distance)
                })
                .collect();
            let mut cosine: Vec<_> = samples
                .iter()
                .map(|sample| {
                    sample.map_or(-1.0, |s| Vec3::from_array(s.normal).dot(toward).max(0.0))
                })
                .collect();
            fill_chart_border(&mut flux, *size);
            fill_chart_border(&mut cosine, *size);
            flux.extend(cosine);
            flux
        })
        .collect();
    let local_samples = crate::renderer::static_ao::bvh::rasterize_lightmap_texels(
        &geometry.receivers,
        &key.local_sizes,
        1,
    );
    let local = local_samples
        .par_iter()
        .zip(&key.local_sizes)
        .map(|(samples, size)| {
            let mut page = Vec::with_capacity(samples.len() * (key.locals.len() + 3));
            let mut totals = vec![[0.0f32; 3]; samples.len()];
            for (_, light) in &key.locals {
                let mut layer: Vec<f32> = samples
                    .par_iter()
                    .map(|sample| {
                        if cancel.load(Ordering::Relaxed) {
                            return -1.0;
                        }
                        let Some(sample) = sample else {
                            return -1.0;
                        };
                        let position = Vec3::from_array(sample.position);
                        let normal = Vec3::from_array(sample.normal);
                        let incident = super::lightmap_local::irradiance(light, position, normal);
                        if incident <= 0.0 {
                            return 0.0;
                        }
                        let origin = position + normal * 0.5;
                        let delta = Vec3::from_array(light.position) - origin;
                        incident * visibility(origin, delta.normalize_or_zero(), delta.length())
                    })
                    .collect();
                fill_chart_border(&mut layer, *size);
                for (total, value) in totals.iter_mut().zip(&layer) {
                    for c in 0..3 { total[c] += value * light.color[c]; }
                }
                page.extend(layer);
            }
            for c in 0..3 { page.extend(totals.iter().map(|total| total[c])); }
            page
        })
        .collect();
    if cancel.load(Ordering::Relaxed) {
        Err("cancelled".into())
    } else {
        Ok(BakeOutput { sun: pages, local })
    }
}

impl Renderer {
    /// Common frame-boundary entry: commands only change inputs. Old results
    /// cannot be used after a sun/material change, even while a worker finishes.
    pub(in crate::renderer) fn sync_static_sun_lightmaps(&mut self) {
        let sun = self.active_sun();
        let bake_sun = self.baked_sun_cache
            && self.baked_sun_debug_mode == 0
            && self.cascaded_shadow_mode.is_csm();
        let light_sources = self.added_map_light_sources();
        let bake_local = light_sources != 0 && self.baked_sun_debug_mode == 0;
        let Some(world) = self.world.as_mut() else {
            return;
        };
        let state = &mut world.static_sun;
        if (!bake_sun && !bake_local) || world.source_map || state.geometry.is_none() {
            state.pending = None;
            state.key = None;
            state.flux.clear();
            state.local_flux.clear();
            state.local_lights.clear();
            return;
        }
        let images = world
            .baked_sun_cache
            .as_ref()
            .map_or(&world.lightmaps, |c| &c.original);
        let same_sizes =
            state.source_sizes.len() == images.len()
                && state.source_sizes.iter().zip(images).all(|(size, image)| {
                    *size == [image._texture.width(), image._texture.height()]
                });
        let requested = state
            .key
            .as_ref()
            .or_else(|| state.pending.as_ref().map(|p| &p.key))
            .or(state.failed.as_ref());
        let same_request = same_sizes
            && requested.is_some_and(|k| {
                k.direction == sun.direction
                    && k.bake_sun == bake_sun
                    && k.local_sources
                        == if bake_local {
                            light_sources
                        } else {
                            0
                        }
                    && k.visibility == self.sun_visibility
                    && k.models == self.draw_bsp_model_surfaces
                    && k.tinted == self.tinted_glass_enabled
            });
        let outcome = if same_request {
            state.pending.as_ref().and_then(|p| match p.rx.try_recv() {
                Ok(value) => Some(value),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("static sunlight worker disconnected".into()))
                }
            })
        } else {
            None
        };
        if same_request && outcome.is_none() {
            return;
        }
        let locals: Vec<_> = if bake_local {
            world
                .static_bake_sources
                .iter()
                .copied()
                .filter(|(_, light)| super::lightmap_local::source_enabled(light, light_sources))
                .take(super::lightmap_local::MAX_SOURCES.min(self.device.limits().max_texture_array_layers.saturating_sub(3) as usize))
                .collect()
        } else {
            Vec::new()
        };
        let base: Vec<_> = images
            .iter()
            .map(|i| [i._texture.width(), i._texture.height()])
            .collect();
        state.source_sizes = base.clone();
        let key = Key {
            direction: sun.direction,
            visibility: self.sun_visibility,
            models: self.draw_bsp_model_surfaces,
            tinted: self.tinted_glass_enabled,
            sizes: if bake_sun {
                output_sizes(&base, self.device.limits().max_texture_dimension_2d / 2)
            } else {
                base.clone()
            },
            bake_sun,
            local_sources: if bake_local {
                light_sources
            } else {
                0
            },
            local_sizes: super::lightmap_local::output_sizes(
                &base,
                locals.len(),
                self.device.limits().max_texture_dimension_2d,
            ),
            locals,
        };
        if key.sizes.is_empty() {
            if state.failed.as_ref() != Some(&key) {
                println!(
                    "[SUN LIGHTMAP] output exceeds the lightmap budget or device limit; retaining live CSM"
                );
            }
            state.failed = Some(key);
            state.pending = None;
            state.key = None;
            state.flux.clear();
            return;
        }
        if state.key.as_ref() == Some(&key) || state.failed.as_ref() == Some(&key) {
            return;
        }
        if state.pending.as_ref().is_some_and(|p| p.key != key) {
            state.pending = None;
        }
        state.key = None;
        state.local_lights.clear();
        if let Some(outcome) = outcome {
            state.pending = None;
            match outcome {
                Ok(pages) => {
                    state.flux = pages
                        .sun
                        .iter()
                        .zip(&key.sizes)
                        .map(|(pixels, size)| {
                            use wgpu::util::DeviceExt;
                            let texture = self.device.create_texture_with_data(
                                &self.queue,
                                &wgpu::TextureDescriptor {
                                    label: Some("static sunlight incident flux"),
                                    size: wgpu::Extent3d {
                                        width: size[0],
                                        height: size[1] * 2,
                                        depth_or_array_layers: 1,
                                    },
                                    mip_level_count: 1,
                                    sample_count: 1,
                                    dimension: wgpu::TextureDimension::D2,
                                    format: wgpu::TextureFormat::R32Float,
                                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                                        | wgpu::TextureUsages::COPY_DST,
                                    view_formats: &[],
                                },
                                wgpu::util::TextureDataOrder::LayerMajor,
                                bytemuck::cast_slice(pixels),
                            );
                            let view = texture.create_view(&Default::default());
                            GpuImage::new(texture, view)
                        })
                        .collect();
                    state.local_flux = pages
                        .local
                        .iter()
                        .zip(&key.local_sizes)
                        .map(|(pixels, size)| {
                            use wgpu::util::DeviceExt;
                            let texture = self.device.create_texture_with_data(
                                &self.queue,
                                &wgpu::TextureDescriptor {
                                    label: Some("baked per-light local irradiance"),
                                    size: wgpu::Extent3d {
                                        width: size[0],
                                        height: size[1],
                                        depth_or_array_layers: (key.locals.len() + 3) as u32,
                                    },
                                    mip_level_count: 1,
                                    sample_count: 1,
                                    dimension: wgpu::TextureDimension::D2,
                                    format: wgpu::TextureFormat::R32Float,
                                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                                        | wgpu::TextureUsages::COPY_DST,
                                    view_formats: &[],
                                },
                                wgpu::util::TextureDataOrder::LayerMajor,
                                bytemuck::cast_slice(pixels),
                            );
                            let view = texture.create_view(&wgpu::TextureViewDescriptor {
                                dimension: Some(wgpu::TextureViewDimension::D2Array),
                                ..Default::default()
                            });
                            GpuImage::new(texture, view)
                        })
                        .collect();
                    state.local_lights = key.locals.clone();
                    println!(
                        "[LOCAL LIGHTMAP] baked {} static emitters into {} page(s), sizes {:?}",
                        key.locals.len(),
                        state.local_flux.len(),
                        key.local_sizes
                    );
                    state.key = Some(key);
                    state.generation = state.generation.wrapping_add(1).max(1);
                    println!(
                        "[SUN LIGHTMAP] static visibility ready, {} page(s), sizes {:?}; camera movement does not rebake",
                        state.flux.len(),
                        state
                            .flux
                            .iter()
                            .map(|i| [i._texture.width(), i._texture.height() / 2])
                            .collect::<Vec<_>>()
                    );
                }
                Err(e) => {
                    eprintln!("[SUN LIGHTMAP] {e}");
                    state.failed = Some(key);
                }
            }
            return;
        }
        if state.pending.is_none() {
            let geometry = state.geometry.clone().unwrap();
            let worker_key = key.clone();
            let cancel = Arc::new(AtomicBool::new(false));
            let worker_cancel = Arc::clone(&cancel);
            let (tx, rx) = mpsc::channel();
            match std::thread::Builder::new()
                .name("static-sun-lightmap".into())
                .spawn(move || {
                    let pool = rayon::ThreadPoolBuilder::new()
                        .num_threads(
                            std::thread::available_parallelism()
                                .map_or(1, usize::from)
                                .saturating_sub(1)
                                .clamp(1, 4),
                        )
                        .build();
                    let result = pool.map_err(|e| e.to_string()).and_then(|pool| {
                        pool.install(|| trace(&geometry, &worker_key, &worker_cancel))
                    });
                    let _ = tx.send(result);
                }) {
                Ok(_) => state.pending = Some(Pending { key, rx, cancel }),
                Err(e) => {
                    eprintln!("[SUN LIGHTMAP] could not start worker: {e}");
                    state.failed = Some(key);
                }
            }
        }
    }
}

impl StaticSun {
    pub(in crate::renderer) fn new(geometry: Option<Geometry>) -> Self {
        Self {
            geometry,
            ..Default::default()
        }
    }
    pub(in crate::renderer) fn replace_geometry(&mut self, geometry: Geometry) {
        *self = Self::new(Some(geometry));
    }
    pub(in crate::renderer) fn local_ready(&self) -> bool {
        self.key.is_some() && !self.local_flux.is_empty() && !self.local_lights.is_empty()
    }
    pub(in crate::renderer) fn ready(&self) -> bool {
        self.key.as_ref().is_some_and(|k| k.bake_sun) && !self.flux.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_resolution_is_independent_and_bounded() {
        assert_eq!(output_sizes(&[[128, 128]], 8192), vec![[512, 512]]);
        assert_eq!(output_sizes(&[[2048, 2048]], 8192), vec![[2048, 2048]]);
        assert_eq!(output_sizes(&[[1024, 1024]], 2048), vec![[2048, 2048]]);
    }
    #[test]
    fn lightmap_sun_traces_static_roofs_and_cancels_obsolete_bakes() {
        let text = r#"{
"classname" "worldspawn"
{
( 0 0 0 ) ( 0 64 0 ) ( 0 0 64 ) test/wall 0 0 0 1 1
( 64 0 0 ) ( 64 0 64 ) ( 64 64 0 ) test/wall 0 0 0 1 1
( 0 0 0 ) ( 0 0 64 ) ( 64 0 0 ) test/wall 0 0 0 1 1
( 0 64 0 ) ( 64 64 0 ) ( 0 64 64 ) test/wall 0 0 0 1 1
( 0 0 0 ) ( 64 0 0 ) ( 0 64 0 ) test/floor 0 0 0 1 1
( 0 0 64 ) ( 0 64 64 ) ( 64 0 64 ) test/ceiling 0 0 0 1 1
}
}"#;
        let map = scene::prepare_source(
            &std::env::temp_dir().join("sun-lightmap-test"),
            None,
            &scene::MapSource::MapEditPreview {
                path: "sun.map".into(),
                text: text.into(),
            },
        )
        .unwrap();
        let mut floor = map.batches[0].clone();
        floor.vertices = 0..3;
        floor.lightmap = Some(0);
        floor.pipeline.class = DrawClass::Opaque;
        floor.pipeline.depth_equal = false;
        floor.alpha_cutoff = 0.0;
        let mut roof = floor.clone();
        roof.vertices = 3..6;
        roof.lightmap = None;
        let mut sky = roof.clone();
        sky.vertices = 6..9;
        sky.pipeline.class = DrawClass::Sky;
        sky.tc_gen = jka_assets::shader::TcGen::Base;
        let positions = [
            [0.0, 0.0, 0.0],
            [100.0, 0.0, 0.0],
            [0.0, 0.0, 100.0],
            [0.0, 50.0, 0.0],
            [50.0, 50.0, 0.0],
            [0.0, 50.0, 50.0],
            [-100.0, 100.0, -100.0],
            [300.0, 100.0, -100.0],
            [-100.0, 100.0, 300.0],
        ];
        let vertices = Arc::new(
            positions
                .into_iter()
                .enumerate()
                .map(|(i, position)| GpuVertex {
                    position,
                    normal: [0.0, 1.0, 0.0],
                    uv: [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]][i % 3],
                    lightmap_uv: [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]][i % 3],
                    color: [1.0; 4],
                    alpha_cutoff: 0.0,
                })
                .collect(),
        );
        let geometry = Geometry::new(
            vertices,
            vec![floor.clone(), roof, sky],
            Arc::new(vec![]),
            &map.material_debug,
        );
        let key = Key {
            direction: [0.0, -1.0, 0.0],
            visibility: SunVisibilityMode::SkyPortals,
            models: true,
            tinted: false,
            sizes: vec![[32, 32]],
            bake_sun: true,
            local_sources: 0,
            local_sizes: Vec::new(),
            locals: Vec::new(),
        };
        let cancel = AtomicBool::new(false);
        let pages = trace(&geometry, &key, &cancel).unwrap().sun;
        assert_eq!(pages[0][4 * 32 + 4], 0.0, "roof blocks the sun");
        assert_eq!(pages[0][4 * 32 + 24], 1.0, "open floor receives the sun");
        assert!(
            pages[0][..32 * 32]
                .iter()
                .zip(&pages[0][32 * 32..])
                .all(|(incident, cosine)| incident <= &(cosine + 1e-6)),
            "chart padding must not add sunlight above its unoccluded cosine"
        );
        assert_eq!(
            pages[0][32 * 32 + 4 * 32 + 4],
            1.0,
            "unoccluded cosine survives shadowing"
        );
        let lamp = scene::DynamicLight {
            position: [10.0, 75.0, 10.0],
            color: [1.0, 0.0, 0.0],
            radius: 200.0,
            intensity: 1.0,
            shadow_power: 0.0,
            falloff: scene::DynamicLightFalloff::Smooth,
            surface_lighting: true,
            emitter_normal: [0.0; 3],
            emitter_two_sided: false,
            angle_attenuation: true,
            angle_scale: 0.0,
            extra_distance: 0.0,
            shader_promotion_mask: 1,
        };
        let mut local_key = key.clone();
        local_key.local_sizes = vec![[32, 32]];
        local_key.locals = vec![
            (0, lamp),
            (
                1,
                scene::DynamicLight {
                    position: [10.0, 25.0, 10.0],
                    ..lamp
                },
            ),
        ];
        let local = trace(&geometry, &local_key, &cancel).unwrap().local;
        let receiver = 4 * 32 + 4;
        assert_eq!(local[0][receiver], 0.0, "roof blocks the upper lamp");
        assert!(
            local[0][32 * 32 + receiver] > 0.0,
            "lower lamp reaches the receiver independently"
        );
        assert_eq!(local[0].len(),32*32*5);
        assert_eq!(local[0][32*32*2+receiver],local[0][32*32+receiver],"aggregate red irradiance is baked once");
        assert_eq!(local[0][32*32*3+receiver],0.0,"red lights add no green irradiance");
        floor.alpha_cutoff = -0.5;
        floor.animation = None;
        floor.tc_mods = vec![jka_assets::shader::TcMod::Scale(2.0, 2.0)];
        assert!(!moving_static_material(
            &floor,
            SunVisibilityMode::SkyPortals
        ));
        floor.tc_mods = vec![jka_assets::shader::TcMod::Scroll(1.0, 0.0)];
        assert!(
            moving_static_material(&floor, SunVisibilityMode::SkyPortals),
            "negative alphaFunc must stay live too"
        );
        cancel.store(true, Ordering::Relaxed);
        assert!(trace(&geometry, &key, &cancel).is_err());
    }
}
