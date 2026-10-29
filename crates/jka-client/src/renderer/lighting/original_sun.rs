//! Reconstruct authored sunlight in the original BSP lightmap UV charts and
//! compiler photon units, then subtract before filtering exposed lightmap texels.
//! Cached map-wide visibility and paired fits remain fallbacks for unknown bake
//! profiles. None of these paths follows the camera or live sun override.
use crate::renderer::{
    aabb_intersects_shadow_frustum, draw_world_batch_secondary, BlendMode,
    DrawClass, Mat4, Renderer, ShadowCasterUniform, Vec3, Vec4, SHADOW_CASCADES,
    SHADOW_MAP_SIZE, TcGen,
};
use crate::renderer::lighting::shadow_resources::OriginalSunShadowCache;

pub(in crate::renderer) struct BakedSunCalibration {
    pub radiance: [f32; 3],
    /// Shadow luminance, direct upper bound, classification, reserved.
    /// Classification: 0 fallback, 1 paired fit, 2 fully lit, 3 fully shadowed,
    /// 4 authored UV projection (exposure, gamma, kind, signed compensation).
    pub surfaces: Vec<[f32; 4]>,
}

struct SunFilterTriangle {
    uv: [[f32; 2]; 3],
    texture: usize,
    alpha: bool,
    color: bool,
}

/// q3map2 traces undeformed BSP UVs against its light image, without runtime
/// alphaFunc thresholds, tcMods, vertex alpha or animation. Each intersected
/// leaf transmits inverse alpha; lightfilter additionally multiplies file RGB.
fn compiler_sun_transmission(
    image: &crate::materials::TextureData,
    uv: [f32; 2],
    alpha: bool,
    color: bool,
) -> Vec3 {
    let x = ((uv[0] - uv[0].floor()) * image.width as f32) as usize;
    let y = ((uv[1] - uv[1].floor()) * image.height as f32) as usize;
    let Some(pixel) = image.rgba.get((y * image.width as usize + x) * 4..)
        .and_then(|bytes| bytes.get(..4)) else { return Vec3::ZERO; };
    let rgb = if color { Vec3::new(pixel[0] as f32, pixel[1] as f32, pixel[2] as f32) / 255.0 }
        else { Vec3::ONE };
    rgb * if alpha { 1.0 - pixel[3] as f32 / 255.0 } else { 1.0 }
}

fn authored_sun_casters(
    vertices: &[crate::scene::GpuVertex],
    batches: &[crate::scene::DrawBatch],
) -> (Vec<crate::renderer::StaticAoRenderTriangle>, Vec<Option<SunFilterTriangle>>) {
    let mut seen = std::collections::BTreeSet::new();
    let mut triangles = Vec::new();
    let mut filters = Vec::new();
    for batch in batches {
        let filtered = batch.alpha_shadow || batch.light_filter;
        if batch.pipeline.class == DrawClass::Sky
            || (!filtered && batch.pipeline.class != DrawClass::Opaque)
            || !seen.insert((batch.vertices.start, batch.vertices.end)) { continue; }
        for triangle in vertices[batch.vertices.start as usize..batch.vertices.end as usize].chunks_exact(3) {
            triangles.push(crate::renderer::StaticAoRenderTriangle {
                positions: [triangle[0].position, triangle[1].position, triangle[2].position],
            });
            filters.push(if filtered { batch.baked_shadow_texture.map(|texture| SunFilterTriangle {
                uv: [triangle[0].uv, triangle[1].uv, triangle[2].uv],
                texture, alpha: batch.alpha_shadow, color: batch.light_filter,
            }) } else { None });
        }
    }
    (triangles, filters)
}

/// Rasterize the authored sunlight into the ORIGINAL lightmap UV charts.
/// Alpha is reserved for incident sun flux (visibility times cosine); RGB is
/// unchanged, so strength zero remains exactly the original map. No screen-space
/// samples, per-surface brightness estimates, or removal-fraction caps participate.
pub(in crate::renderer) fn bake_authored_sun_masks(
    vertices: &[crate::scene::GpuVertex],
    batches: &[crate::scene::DrawBatch],
    textures: &[crate::materials::TextureData],
    lightmaps: &mut [crate::materials::TextureData],
    surface_ids: &[u32],
    surface_count: usize,
    sun: crate::scene::DirectionalSun,
    encoding: crate::scene::BakedSunEncoding,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Option<BakedSunCalibration> {
    use crate::renderer::{
        collect_static_ao_lightmap_triangles, StaticAoBvh, StaticAoRenderTriangle,
    };
    use rayon::prelude::*;
    let started = std::time::Instant::now();
    if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    let (casters, filters) = authored_sun_casters(vertices, batches);
    let filter_count = filters.iter().filter(|filter| filter.is_some()).count();
    let solid = StaticAoBvh::build(casters.into()).ok()?;
    println!(
        "[BAKED SUN] {} compiler-filtered caster triangles",
        filter_count
    );
    let mut sky_triangles = Vec::new();
    for batch in batches
        .iter()
        .filter(|b| b.pipeline.class == DrawClass::Sky && !matches!(b.tc_gen, TcGen::SkyCloud(_)))
    {
        for triangle in
            vertices[batch.vertices.start as usize..batch.vertices.end as usize].chunks_exact(3)
        {
            sky_triangles.push(StaticAoRenderTriangle {
                positions: [
                    triangle[0].position,
                    triangle[1].position,
                    triangle[2].position,
                ],
            });
        }
    }
    let sky = StaticAoBvh::build(sky_triangles.into()).ok()?;
    let triangles = collect_static_ao_lightmap_triangles(vertices, batches);
    let sizes: Vec<_> = lightmaps.iter().map(|lm| [lm.width, lm.height]).collect();
    let samples = crate::renderer::static_ao::bvh::rasterize_lightmap_texels(&triangles, &sizes, 1);
    let owners = crate::renderer::static_ao::bvh::rasterize_lightmap_owners(&triangles, &sizes, 1);
    let color = Vec3::from_array(sun.color);
    let photons = color / color.max_element().max(0.00001) * sun.intensity * encoding.brightness;
    let sun_y = photons.dot(Vec3::new(0.2126, 0.7152, 0.0722));
    let refined = std::sync::atomic::AtomicUsize::new(0);
    let toward = -Vec3::from_array(sun.direction).normalize_or_zero();
    let azimuth = (-toward.z).atan2(toward.x);
    let elevation = toward.y.asin();
    let count = encoding.samples.max(1);
    let directions: Vec<_> = (0..count)
        .map(|i| {
            if i == 0 || encoding.deviance <= 0.0 {
                return toward;
            }
            // Deterministic disk quadrature in the same azimuth/elevation domain as
            // q3map_sunExt. The compiler's random seed is not stored in the BSP.
            let radius = ((i as f32 - 0.5) / (count - 1).max(1) as f32).sqrt()
                * encoding.deviance.to_radians();
            let phase = i as f32 * 2.3999632;
            let a = azimuth + radius * phase.cos();
            let e = elevation + radius * phase.sin();
            Vec3::new(a.cos() * e.cos(), e.sin(), -a.sin() * e.cos())
        })
        .collect();
    let traced: usize = lightmaps
        .par_iter_mut()
        .zip(samples.par_iter())
        .zip(owners.par_iter())
        .map(|((image, texels), owners)| {
            if image.rgba16f.is_some() {
                return 0;
            }
            // A map commonly has ONE packed atlas. Parallelizing pages alone left
            // all its sunlight rays on a single core. Independent texel blocks keep
            // the exact sample sequence/math while sharing immutable BVHs.
            let traced: usize = image
                .rgba
                .par_chunks_mut(256 * 4)
                .zip(texels.par_chunks(256))
                .map(|(pixels, texels)| {
                    if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                        return 0;
                    }
                    let mut traced = 0;
                    for (pixel, sample) in pixels.chunks_exact_mut(4).zip(texels) {
                        pixel[3] = 0;
                        let Some(sample) = sample else {
                            continue;
                        };
                        let normal = Vec3::from_array(sample.normal);
                        let position = Vec3::from_array(sample.position);
                        let mut flux = 0.0;
                        for &direction in &directions {
                            let facing = normal.dot(direction).max(0.0);
                            if facing <= 0.0 {
                                continue;
                            }
                            let origin = position + normal * 1.0;
                            let Some(sky_distance) = sky.trace(origin, direction, 131072.0) else {
                                continue;
                            };
                            let mut transmission = Vec3::ONE;
                            let blocked = solid
                                .trace_filtered(
                                    origin,
                                    direction,
                                    (sky_distance - 0.1).max(0.0),
                                    |index, bary| {
                                        let Some(filter) = &filters[index] else {
                                            return true;
                                        };
                                        let Some(image) = textures.get(filter.texture) else {
                                            return true;
                                        };
                                        let uv = [0, 1].map(|axis| {
                                            (0..3).map(|i| filter.uv[i][axis] * bary[i]).sum()
                                        });
                                        transmission *= compiler_sun_transmission(
                                            image,
                                            uv,
                                            filter.alpha,
                                            filter.color,
                                        );
                                        transmission.max_element() <= 0.001
                                    },
                                )
                                .is_some();
                            if !blocked {
                                // The lightmap mask stores scalar flux. Preserve the energy
                                // of colored transmission relative to this authored sun.
                                flux += facing
                                    * (photons * transmission)
                                        .dot(Vec3::new(0.2126, 0.7152, 0.0722))
                                    / sun_y.max(0.00001);
                            }
                        }
                        pixel[3] = (flux / count as f32 * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
                        traced += 1;
                    }
                    traced
                })
                .sum();
            if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                return 0;
            }
            refined.fetch_add(
                refine_authored_sun_edges(
                    image, texels, owners, toward, sun_y, encoding, cancelled,
                ),
                std::sync::atomic::Ordering::Relaxed,
            );
            // Match bilinear sampling at chart edges without bleeding another chart.
            let alpha: Vec<_> = image.rgba.chunks_exact(4).map(|p| p[3]).collect();
            for (index, sample) in texels.iter().enumerate() {
                if sample.is_some() {
                    continue;
                }
                let x = index % image.width as usize;
                let y = index / image.width as usize;
                let mut sum = 0u32;
                let mut count = 0u32;
                for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx < 0 || ny < 0 || nx >= image.width as i32 || ny >= image.height as i32 {
                        continue;
                    }
                    let neighbor = ny as usize * image.width as usize + nx as usize;
                    if texels[neighbor].is_some() {
                        sum += alpha[neighbor] as u32;
                        count += 1;
                    }
                }
                if count > 0 {
                    image.rgba[index * 4 + 3] = (sum / count) as u8;
                }
            }
            traced
        })
        .sum();
    if traced == 0 || cancelled.load(std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    let mut surfaces = vec![[0.0; 4]; surface_count];
    for batch in batches {
        let Some(image) = batch
            .lightmap
            .and_then(|page| lightmaps.get(page))
            .filter(|lm| lm.rgba16f.is_none())
        else {
            continue;
        };
        for start in (batch.vertices.start as usize..batch.vertices.end as usize).step_by(3) {
            if let Some(fit) = surface_ids
                .get(start / 3)
                .and_then(|&id| surfaces.get_mut(id as usize))
            {
                *fit = [
                    encoding.exposure,
                    encoding.gamma,
                    4.0,
                    encoding.compensate * if image.srgb { 1.0 } else { -1.0 },
                ];
            }
        }
    }
    // Q3Map2 ColorNormalize divides by the brightest channel, not vector length.
    let color = Vec3::from_array(sun.color);
    let radiance =
        color / color.max_element().max(0.00001) * sun.intensity * encoding.brightness / 255.0;
    println!("[BAKED SUN] authored UV projection: {} luxels, {} sun samples, intensity {}, exposure {}, gamma {}, {} edge luxels refined, {:.1} ms",
        traced, count, sun.intensity, encoding.exposure, encoding.gamma, refined.load(std::sync::atomic::Ordering::Relaxed), started.elapsed().as_secs_f64() * 1000.0);
    Some(BakedSunCalibration {
        radiance: radiance.to_array(),
        surfaces,
    })
}

/// Inverse of Q3Map2's exposed or default linear output transform. Input is
/// the file bytes (before the GPU's sRGB decode), output is compiler photon units.
fn compiled_lightmap_photons(pixel: &[u8], encoding: crate::scene::BakedSunEncoding) -> Vec3 {
    let encoded = Vec3::new(pixel[0] as f32, pixel[1] as f32, pixel[2] as f32) * (encoding.compensate / 255.0);
    let peak = encoded.max_element();
    if peak <= 0.0 { return Vec3::ZERO; }
    // With zero exposure, unsaturated bytes retain compiler units directly.
    // Saturated bytes provide a lower bound; their clipped energy is unrecoverable.
    let gamma_rgb = if encoding.exposure > 0.0 {
        let raw_peak = -encoding.exposure * (1.0 - peak.min(254.5 / 255.0)).ln();
        encoded / peak * (raw_peak / 255.0)
    } else { encoded };
    Vec3::new(gamma_rgb.x.powf(encoding.gamma), gamma_rgb.y.powf(encoding.gamma), gamma_rgb.z.powf(encoding.gamma)) * 255.0
}

/// Recover the compiler's filtered visibility only near a traced shadow edge.
/// Fully lit/shadowed neighboring luxels constrain the surviving light field;
/// known authored sun energy converts the observed residual into visibility.
/// Atlas ownership and plane tests prevent borrowing a different surface/chart.
fn refine_authored_sun_edges(
    image: &mut crate::materials::TextureData,
    texels: &[Option<crate::renderer::StaticAoTexelSample>],
    owners: &[u32],
    toward: Vec3,
    sun_y: f32,
    encoding: crate::scene::BakedSunEncoding,
    cancelled: &std::sync::atomic::AtomicBool,
) -> usize {
    use rayon::prelude::*;
    if sun_y <= 0.0 {
        return 0;
    }
    let width = image.width as i32;
    let height = image.height as i32;
    let alpha: Vec<_> = image
        .rgba
        .chunks_exact(4)
        .map(|p| p[3] as f32 / 255.0)
        .collect();
    let raw: Vec<_> = image
        .rgba
        .chunks_exact(4)
        .map(|p| compiled_lightmap_photons(p, encoding).dot(Vec3::new(0.2126, 0.7152, 0.0722)))
        .collect();
    let mut kinds = vec![-1i8; texels.len()];
    for (index, sample) in texels.iter().enumerate() {
        let Some(sample) = sample else {
            continue;
        };
        let facing = Vec3::from_array(sample.normal).dot(toward).max(0.0);
        if facing < 0.1 {
            continue;
        }
        kinds[index] = if alpha[index] < 0.005 {
            0
        } else if (alpha[index] - facing).abs() < 0.012 {
            1
        } else {
            2
        };
    }
    let neighbor = |x: i32, y: i32| -> Option<usize> {
        (x >= 0 && y >= 0 && x < width && y < height).then(|| (y * width + x) as usize)
    };
    let mut stable = vec![false; texels.len()];
    let mut edge = vec![false; texels.len()];
    for (index, &kind) in kinds.iter().enumerate() {
        if kind < 0 {
            continue;
        }
        let x = index as i32 % width;
        let y = index as i32 / width;
        let mut matching = 0;
        let mut differs = kind == 2;
        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            if let Some(n) =
                neighbor(x + dx, y + dy).filter(|&n| owners[n] == owners[index] && kinds[n] >= 0)
            {
                if kinds[n] == kind {
                    matching += 1;
                } else {
                    differs = true;
                }
            }
        }
        stable[index] = kind != 2 && matching >= 3 && !differs;
        edge[index] = differs;
    }
    image
        .rgba
        .par_chunks_mut(256 * 4)
        .enumerate()
        .map(|(block, pixels)| {
            if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                return 0;
            }
            let mut refined = 0;
            for (local, pixel) in pixels.chunks_exact_mut(4).enumerate() {
                let index = block * 256 + local;
                let sample = &texels[index];
                let Some(sample) = sample else {
                    continue;
                };
                if kinds[index] < 0 {
                    continue;
                }
                let x = index as i32 % width;
                let y = index as i32 / width;
                if !(-2..=2).any(|dy| {
                    (-2..=2).any(|dx| {
                        neighbor(x + dx, y + dy)
                            .is_some_and(|n| owners[n] == owners[index] && edge[n])
                    })
                }) {
                    continue;
                }
                let normal = Vec3::from_array(sample.normal);
                let position = Vec3::from_array(sample.position);
                let mut baseline = 0.0;
                let mut weights = 0.0;
                let mut lit_count = 0;
                let mut shade_count = 0;
                for dy in -6..=6 {
                    for dx in -6..=6 {
                        let Some(n) = neighbor(x + dx, y + dy)
                            .filter(|&n| stable[n] && owners[n] == owners[index])
                        else {
                            continue;
                        };
                        let other = texels[n].as_ref().unwrap();
                        if normal.dot(Vec3::from_array(other.normal)) < 0.99
                            || normal
                                .dot(Vec3::from_array(other.position) - position)
                                .abs()
                                > 0.5
                        {
                            continue;
                        }
                        let residual = raw[n] - sun_y * alpha[n];
                        if residual < 0.0 {
                            continue;
                        }
                        let weight = 1.0 / (1 + dx * dx + dy * dy) as f32;
                        baseline += residual * weight;
                        weights += weight;
                        if kinds[n] == 0 {
                            shade_count += 1;
                        } else {
                            lit_count += 1;
                        }
                    }
                }
                if lit_count < 2 || shade_count < 2 {
                    continue;
                }
                baseline /= weights;
                let flux =
                    ((raw[index] - baseline) / sun_y).clamp(0.0, normal.dot(toward).max(0.0));
                let value = (flux * 255.0).round() as u8;
                if pixel[3] != value {
                    refined += 1;
                }
                pixel[3] = value;
            }
            refined
        })
        .sum()
}

/// Estimate the compiled direct term, rather than treating compiler sun units
/// as linear texture values. Each comparison is within ONE authored surface;
/// triangle-interior samples cannot read a different packed lightmap chart.
/// Only surfaces containing both ray-tested sunlight and shadow constrain the
/// fit. A robust median rejects lamps, bounce gradients and atypical surfaces.
pub(in crate::renderer) fn estimate_baked_sun_radiance(
    vertices: &[crate::scene::GpuVertex],
    batches: &[crate::scene::DrawBatch],
    lightmaps: &[crate::materials::TextureData],
    surface_ids: &[u32],
    surfaces: &[crate::scene::LegacyDlightSurface],
    sun: crate::scene::DirectionalSun,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Option<BakedSunCalibration> {
    use crate::renderer::{collect_static_ao_render_triangles, StaticAoBvh};
    let toward = -Vec3::from_array(sun.direction).normalize_or_zero();
    let hue = Vec3::from_array(sun.color).max(Vec3::ZERO);
    let luminance = |rgb: Vec3| rgb.dot(Vec3::new(0.2126, 0.7152, 0.0722));
    if sun.intensity <= 0.0 || luminance(hue) <= 0.00001 { return None; }
    let hue = hue / luminance(hue);
    let triangles = collect_static_ao_render_triangles(vertices, batches);
    let bvh = StaticAoBvh::build(triangles.into()).ok()?;
    let mut faces = vec![Vec::<(usize, usize, f32)>::new(); surfaces.len()];
    let mut seen = vec![false; vertices.len() / 3];
    for batch in batches {
        let Some(page) = batch.lightmap.filter(|&page| page < lightmaps.len()) else { continue; };
        if batch.pipeline.class == DrawClass::Sky { continue; }
        for start in (batch.vertices.start as usize..batch.vertices.end as usize).step_by(3) {
            let triangle = start / 3;
            let Some(&surface) = surface_ids.get(triangle) else { continue; };
            let Some(face) = faces.get_mut(surface as usize) else { continue; };
            if start + 2 >= vertices.len() || seen[triangle] { continue; }
            let normal = Vec3::from_array(surfaces[surface as usize].plane[..3].try_into().unwrap());
            if normal.dot(toward) < 0.2 { continue; }
            let p = [0, 1, 2].map(|i| Vec3::from_array(vertices[start + i].position));
            let area = (p[1] - p[0]).cross(p[2] - p[0]).length() * 0.5;
            if area < 0.01 { continue; }
            seen[triangle] = true;
            face.push((start, page, area));
        }
    }
    let mut estimates = Vec::new();
    let mut surface_fits = vec![[0.0; 4]; surfaces.len()];
    for (surface, triangles) in faces.iter().enumerate() {
        if cancelled.load(std::sync::atomic::Ordering::Relaxed) { return None; }
        let area: f32 = triangles.iter().map(|t| t.2).sum();
        if area < 1024.0 { continue; }
        let normal = Vec3::from_array(surfaces[surface].plane[..3].try_into().unwrap());
        let facing = normal.dot(toward);
        let mut lit = Vec3::ZERO;
        let mut shadow = Vec3::ZERO;
        let mut lit_count = 0;
        let mut lit_square_sum = 0.0;
        let mut shadow_count = 0;
        for sample in 0..96 {
            let target = (sample as f32 + 0.5) / 96.0 * area;
            let mut accumulated = 0.0;
            let &(start, page, _) = triangles.iter().find(|t| {
                accumulated += t.2;
                accumulated >= target
            }).unwrap_or_else(|| triangles.last().unwrap());
            // Low-discrepancy barycentric samples, strictly inside the triangle.
            let u = ((sample as f32 + 0.5) * 0.754877666).fract().sqrt();
            let v = ((sample as f32 + 0.5) * 0.569840296).fract();
            let weights = [1.0 - u, u * (1.0 - v), u * v];
            let mut position = Vec3::ZERO;
            let mut uv = glam::Vec2::ZERO;
            for i in 0..3 {
                position += Vec3::from_array(vertices[start + i].position) * weights[i];
                uv += glam::Vec2::from_array(vertices[start + i].lightmap_uv) * weights[i];
            }
            let image = &lightmaps[page];
            if image.rgba16f.is_some() || image.width == 0 || image.height == 0
                || uv.min_element() < 0.0 || uv.max_element() > 1.0 { continue; }
            let x = ((uv.x * image.width as f32) as usize).min(image.width as usize - 1);
            let y = ((uv.y * image.height as f32) as usize).min(image.height as usize - 1);
            let Some(pixel) = image.rgba.get((y * image.width as usize + x) * 4..)
                .filter(|p| p.len() >= 3) else { continue; };
            let rgb = Vec3::from_array([pixel[0], pixel[1], pixel[2]].map(|c| {
                let c = c as f32 / 255.0;
                if image.srgb {
                    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
                } else { c }
            }));
            if bvh.trace(position + normal * 2.0 + toward * 2.0, toward, 131072.0).is_some() {
                shadow += rgb;
                shadow_count += 1;
            } else {
                lit += rgb;
                lit_count += 1;
                lit_square_sum += luminance(rgb).powi(2);
            }
        }
        if lit_count + shadow_count >= 80 {
            if shadow_count == 0 { surface_fits[surface] = [0.0, 0.0, 2.0, 0.0]; }
            if lit_count == 0 { surface_fits[surface] = [0.0, 0.0, 3.0, 0.0]; }
        }
        if lit_count < 8 || shadow_count < 8 { continue; }
        let delta = lit / lit_count as f32 - shadow / shadow_count as f32;
        let energy = luminance(delta);
        if energy < 0.015 { continue; }
        // Strongly colored local lamps are not evidence for the authored sun.
        if (delta / energy - hue).length() > 0.65 { continue; }
        let baseline = luminance(shadow / shadow_count as f32);
        let lit_mean = luminance(lit / lit_count as f32);
        let lit_deviation = (lit_square_sum / lit_count as f32 - lit_mean * lit_mean).max(0.0).sqrt();
        surface_fits[surface] = [baseline, energy + 2.0 * lit_deviation, 1.0, 0.0];
        estimates.push((energy / facing, area.sqrt()));
    }
    let fitted = fitted_direct_energy(&mut estimates)?;
    println!("[BAKED SUN] calibrated from {} mixed sun/shadow surfaces: linear direct RGB {:?}",
        estimates.len(), (hue * fitted).to_array());
    Some(BakedSunCalibration { radiance: (hue * fitted).to_array(), surfaces: surface_fits })
}

fn fitted_direct_energy(estimates: &mut [(f32, f32)]) -> Option<f32> {
    if estimates.len() < 3 { return None; }
    estimates.sort_by(|a, b| a.0.total_cmp(&b.0));
    let half = estimates.iter().map(|e| e.1).sum::<f32>() * 0.5;
    let mut weight = 0.0;
    estimates.iter().find_map(|&(energy, w)| {
        weight += w;
        (weight >= half).then_some(energy)
    })
}

#[cfg(test)]
mod calibration_tests {
    use super::fitted_direct_energy;

    #[test]
    fn baked_sun_foliage_uses_compiler_alpha_color_and_wrapped_uvs() {
        use super::{compiler_sun_transmission, Vec3};
        let image = crate::materials::TextureData { label: "compiler leaf image".into(), source: None,
            width: 2, height: 1, rgba: (vec![64,128,255,0, 255,255,255,128]).into(), rgba16f: None,
            mip_level_count: 1, clamp: true, srgb: true };
        // Compiler samples wrap, even if the runtime stage is clamped, and
        // continuous inverse alpha differs from the renderer's GE128 cutoff.
        let partial = compiler_sun_transmission(&image, [-0.25, 2.0], true, false);
        assert!((partial - Vec3::splat(127.0 / 255.0)).abs().max_element() < 1e-6);
        let filtered = compiler_sun_transmission(&image, [1.25, 0.5], true, true);
        assert!((filtered - Vec3::new(64.0,128.0,255.0) / 255.0).abs().max_element() < 1e-6);
    }

    #[test]
    fn baked_sun_foliage_holes_continue_to_leaves_and_solid_geometry() {
        use super::Vec3;
        use crate::renderer::{StaticAoBvh, StaticAoRenderTriangle};
        let triangles = [1.0, 2.0, 3.0].map(|z| StaticAoRenderTriangle {
            positions: [[-1.0,-1.0,z], [1.0,-1.0,z], [0.0,1.0,z]],
        }).to_vec();
        let bvh = StaticAoBvh::build(triangles.into()).unwrap();
        let mut transmission = 1.0;
        let hit = bvh.trace_filtered(Vec3::ZERO, Vec3::Z, 2.5, |index, bary| {
            assert!((bary[0] - 0.25).abs() < 1e-6);
            assert!((bary[1] - 0.25).abs() < 1e-6);
            assert!((bary[2] - 0.5).abs() < 1e-6);
            transmission *= [1.0, 0.25, 0.0][index];
            false
        });
        assert!(hit.is_none());
        assert_eq!(transmission, 0.25, "transparent hole must not hide a farther leaf");
        let hit = bvh.trace_filtered(Vec3::ZERO, Vec3::Z, 4.0, |index, _| index == 2);
        assert_eq!(hit, Some(3.0), "holes must not hide a farther opaque building");
        assert_eq!(bvh.trace(Vec3::ZERO, Vec3::Z, 4.0), Some(1.0), "AO keeps opaque tracing semantics");
    }

    #[test]
    fn baked_sun_default_encoding_retains_unsaturated_photon_units() {
        let mut encoding = crate::scene::BakedSunEncoding { exposure: 0.0, gamma: 1.0, compensate: 1.0, brightness: 1.0, deviance: 0.0, samples: 1 };
        let raw = super::compiled_lightmap_photons(&[80, 150, 240, 255], encoding);
        assert!((raw - super::Vec3::new(80.0, 150.0, 240.0)).abs().max_element() < 0.0001);
        encoding.gamma = 2.0;
        encoding.compensate = 2.0;
        let raw = super::compiled_lightmap_photons(&[40, 75, 120, 255], encoding);
        let expected = super::Vec3::new(80.0, 150.0, 240.0).powf(2.0) / 255.0;
        assert!((raw - expected).abs().max_element() < 0.0001);
    }

    #[test]
    fn authored_edge_recovery_follows_baked_visibility_without_crossing_charts() {
        use super::{refine_authored_sun_edges, Vec3};
        let encoding = crate::scene::BakedSunEncoding { exposure: 260.0, gamma: 1.0, compensate: 1.0, brightness: 1.0, deviance: 0.0, samples: 1 };
        let mut rgba = Vec::new();
        let mut texels = Vec::new();
        for y in 0..16 { for x in 0..16 {
            let raw = if x < 8 { 100.0_f32 } else { 600.0 };
            let byte = ((1.0 - (-raw / 260.0).exp()) * 255.0).round() as u8;
            // Geometric edge is deliberately displaced one luxel into shadow.
            rgba.extend_from_slice(&[byte, byte, byte, if x < 7 { 0 } else { 255 }]);
            texels.push(Some(crate::renderer::StaticAoTexelSample { position: [x as f32 * 16.0, 0.0, y as f32 * 16.0], normal: [0.0, 1.0, 0.0] }));
        } }
        let original = rgba.clone();
        let make_image = || crate::materials::TextureData { label: "inverse sun edge fixture".into(), source: None, width: 16, height: 16, rgba: (original.clone()).into(), rgba16f: None, mip_level_count: 1, clamp: true, srgb: true };
        let mut image = make_image();
        assert!(refine_authored_sun_edges(&mut image, &texels, &vec![1; 256], Vec3::Y, 500.0, encoding, &std::sync::atomic::AtomicBool::new(false)) > 0);
        assert_eq!(image.rgba[(8 * 16 + 7) * 4 + 3], 0, "displaced edge must not subtract direct sun from baked shadow");
        assert_eq!(image.rgba[(8 * 16 + 15) * 4 + 3], 255, "lit interior remains ray traced");
        for (before, after) in original.chunks_exact(4).zip(image.rgba.chunks_exact(4)) { assert_eq!(&before[..3], &after[..3], "original lightmap RGB must be unchanged"); }
        let mut image = make_image();
        let owners: Vec<_> = (0..256).map(|index| if index % 16 < 8 { 1 } else { 2 }).collect();
        refine_authored_sun_edges(&mut image, &texels, &owners, Vec3::Y, 500.0, encoding, &std::sync::atomic::AtomicBool::new(false));
        assert_eq!(image.rgba[(8 * 16 + 7) * 4 + 3], 255, "another packed chart cannot supply edge-fit anchors");
    }

    #[test]
    fn baked_sun_alpha_survives_old_opaque_ao_cache() {
        let base = crate::renderer::StaticAoBaseLightmap { label: "sun mask".into(), width: 2, height: 1,
            rgba: vec![10,20,30,0, 40,50,60,255].into(), clamp: true, srgb: true };
        let mut cached = [1,2,3,255].repeat(4);
        crate::renderer::static_ao::bake::restore_baked_sun_alpha(&base, &mut cached, 4, 1);
        assert_eq!(cached.chunks_exact(4).map(|p| p[3]).collect::<Vec<_>>(), [0,64,191,255]);
        assert!(cached.chunks_exact(4).all(|p| p[..3] == [1,2,3]), "AO RGB must remain unchanged");
    }

    #[test]
    fn robust_fit_rejects_a_bright_local_light_outlier() {
        let mut samples = [(0.4, 10.0), (0.42, 10.0), (0.41, 10.0), (3.0, 5.0)];
        assert_eq!(fitted_direct_energy(&mut samples), Some(0.41));
    }

    #[test]
    fn insufficient_shadow_pairs_do_not_claim_a_calibration() {
        assert_eq!(fitted_direct_energy(&mut [(0.4, 10.0)]), None);
    }
}

/// Full-map orthographic reverse-Z projection. Excludes skybox-only geometry
/// from the receiver bounds; sky geometry still renders to the sky-admission
/// slice and the special sky vertex shader handles its depth separately.
fn map_sun_projection(world: &super::super::world::types::WorldGpu, sun_travel: Vec3) -> Option<(Mat4, f32)> {
    if !sun_travel.is_finite() || sun_travel.length_squared() < 0.5 {
        return None;
    }
    let mut low = Vec3::splat(f32::MAX);
    let mut high = Vec3::splat(-f32::MAX);
    let mut found = false;
    for batch in &world.coarse_batches {
        if batch.is_inline_entity || batch.source.pipeline.class == DrawClass::Sky
            || batch.ocean_clipmap.is_some() || batch.source.water {
            continue;
        }
        let a = Vec3::from_array(batch.bounds_min);
        let b = Vec3::from_array(batch.bounds_max);
        if !a.is_finite() || !b.is_finite() || !a.cmple(b).all() { continue; }
        low = low.min(a);
        high = high.max(b);
        found = true;
    }
    if !found { return None; }
    let up = if sun_travel.dot(Vec3::Y).abs() > 0.95 { Vec3::Z } else { Vec3::Y };
    let light_from_world = glam::camera::rh::view::look_to_mat4(Vec3::ZERO, sun_travel.normalize(), up);
    let mut lmin = Vec3::splat(f32::MAX);
    let mut lmax = Vec3::splat(-f32::MAX);
    for x in [low.x, high.x] {
        for y in [low.y, high.y] {
            for z in [low.z, high.z] {
                let v = light_from_world.transform_point3(Vec3::new(x, y, z));
                lmin = lmin.min(v);
                lmax = lmax.max(v);
            }
        }
    }
    // 32 units for near/caster bias, 4 pixels around the XY footprint.
    // Static meshes at the BSP's own boundary must not be clipped away.
    let width = (lmax.x - lmin.x).max(64.0) + 64.0;
    let height = (lmax.y - lmin.y).max(64.0) + 64.0;
    let diameter = width.max(height);
    let depth = (lmax.z - lmin.z).max(64.0) + 64.0;
    let texel = diameter / SHADOW_MAP_SIZE as f32;
    let center = Vec3::new(
        ((lmin.x + lmax.x) * 0.5 / texel).round() * texel,
        ((lmin.y + lmax.y) * 0.5 / texel).round() * texel,
        lmax.z + 32.0,
    );
    // Same reverse-Z convention and matrix construction as cascades.rs.
    let translation = Mat4::from_translation(-center);
    let projection = Mat4::from_cols(
        Vec4::new(2.0 / diameter, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 / diameter, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0 / depth, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 1.0),
    );
    Some((projection * translation * light_from_world, texel))
}

impl Renderer {
    /// Draws only on first enable / BSP change / authored sun change.
    /// Uses existing depth-only caster pipelines and a reserved 5th array slice;
    /// no extra draw or texture allocation on frames with a warm cache.
    pub(in crate::renderer) fn encode_original_sun_shadow(
        &mut self, encoder: &mut wgpu::CommandEncoder, camera_pos_time: [f32; 4],
    ) {
        if !self.baked_sun_shadow_map
            || (self.baked_sun_removal <= 0.0 && self.baked_sun_debug_mode != 2)
        {
            return;
        }
        let Some(world) = self.world.as_ref().filter(|world| !world.source_map) else {
            return;
        };
        let Some(sun) = world.sun else { return; };
        let cache_valid = self.shadow_resources.original_sun_cache.is_some_and(|cache| {
            cache.map_identity == world.remap_asset_hash && cache.direction == sun.direction
        });
        if cache_valid { return; }
        let Some((matrix, texel_size)) = map_sun_projection(world, Vec3::from_array(sun.direction)) else {
            self.shadow_resources.original_sun_cache = None;
            self.update_lighting_settings();
            log::warn!("BakedSun: authored-sun visibility unavailable for map hash {}; shadow-map subtraction is fail-closed", world.remap_asset_hash);
            return;
        };
        let slot = SHADOW_CASCADES;
        self.queue.write_buffer(&self.shadow_resources.caster_buffers[slot], 0,
            bytemuck::bytes_of(&ShadowCasterUniform {
                view_proj: matrix.to_cols_array_2d(), camera_pos_time,
            }));
        // Sky admission is not optional. A plain depth map would wrongly
        // subtract sunlight from rooms under a roof with no sky access.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("JKA ORIGINAL baked sun sky admission (cached)"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_resources.sky_layer_views[slot],
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
            pass.set_pipeline(&self.shadow_resources.bevy_sky_pipeline);
            pass.set_bind_group(0, &self.shadow_resources.caster_bind_groups[slot], &[]);
            pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
            pass.set_vertex_buffer(1, world.legacy_dlight_surface_id_buffer.slice(..));
            pass.set_index_buffer(world.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            for batch in &world.coarse_batches {
                if batch.is_inline_entity || batch.source.pipeline.class != DrawClass::Sky
                    || matches!(batch.source.tc_gen, TcGen::SkyCloud(_)) { continue; }
                draw_world_batch_secondary(&mut pass, batch, 0..1, None);
            }
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("JKA ORIGINAL baked sun shadow (cached)"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_resources.layer_views[slot],
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
            pass.set_pipeline(&self.shadow_resources.bevy_pipeline);
            pass.set_bind_group(0, &self.shadow_resources.caster_bind_groups[slot], &[]);
            pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
            pass.set_vertex_buffer(1, world.legacy_dlight_surface_id_buffer.slice(..));
            pass.set_index_buffer(world.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            let mut is_mask = false;
            for batch in &world.coarse_batches {
                if batch.is_inline_entity || batch.source.pipeline.depth_equal
                    || batch.ocean_clipmap.is_some() || batch.source.water
                    || batch.source.pipeline.class == DrawClass::Sky
                    || batch.source.pipeline.blend != BlendMode::Opaque
                    || !aabb_intersects_shadow_frustum(batch.bounds_min, batch.bounds_max, matrix, true)
                { continue; }
                match batch.source.pipeline.class {
                    DrawClass::Opaque if batch.source.alpha_cutoff == 0.0 => {
                        if is_mask { pass.set_pipeline(&self.shadow_resources.bevy_pipeline); is_mask = false; }
                        draw_world_batch_secondary(&mut pass, batch, 0..1, None);
                    }
                    DrawClass::Mask if batch.source.alpha_cutoff != 0.0 => {
                        if !is_mask { pass.set_pipeline(&self.shadow_resources.bevy_mask_pipeline); is_mask = true; }
                        pass.set_bind_group(1, &batch.bind_group, &[]);
                        draw_world_batch_secondary(&mut pass, batch, 0..1, None);
                    }
                    _ => {}
                }
            }
        }
        self.shadow_resources.original_sun_cache = Some(OriginalSunShadowCache {
            map_identity: world.remap_asset_hash,
            direction: sun.direction,
            view_proj: matrix,
            texel_size,
        });
        self.update_lighting_settings();
        println!("[BAKED SUN] cached authored-sun shadow & sky map, size {}x{}, world texel {:.2}u, direction {:?}",
            SHADOW_MAP_SIZE, SHADOW_MAP_SIZE, texel_size, sun.direction);
    }
}
