//! Worker-built promotion of legacy EFX visuals into transient lights.
//!
//! Jedi Academy content often draws visually emissive fire/electricity/flash
//! primitives without an authored `Light {}` component.  Promotion is split in
//! two phases:
//!
//! * map-load worker: scan every visible `.efx`, pick at most one driver
//!   primitive, resolve its shader stages, decode/sample the referenced image
//!   once, and build an immutable catalog;
//! * frame time: look up the already-built profile and emit/merge a transient
//!   light.  No texture decoding or image scan is needed on the normal hot path.
//!
//! Explicit `Light {}` components always win and are never augmented.

use super::{
    draw::{FxBlend, FxMaterial},
    system::{FxAutoLightCandidate, FxLight, FxLightKind},
    template::{self, AutoLightProfile, PrimType},
};
use crate::{materials::Textures, scene};
use jka_assets::{
    pk3::AssetSearchPath,
    shader::{AlphaGen, RgbGen, Shader},
};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

const FINAL_PROMOTION_SCORE: i16 = 7;
const AGGRESSIVE_PROMOTION_SCORE: i16 = 3;
const PROMOTED_LIGHT_MIN_LIFE_MS: i32 = 40;
const MAX_EFX_BYTES: usize = 65_536;
const MAX_TEXTURE_SAMPLES: usize = 4096;
const MAX_PROMOTED_LIGHTS: usize = 64;

#[derive(Clone, Debug)]
struct ProjectileLightDriver {
    kind: PrimType,
    shader: String,
    score: i16,
    radius: f32,
    rgba: [u8; 4],
}

#[derive(Clone, Copy, Debug)]
struct MaterialLightAnalysis {
    score: i16,
    texture_color: [f32; 3],
    rgb_vertex: bool,
    rgb_const: [f32; 3],
    alpha_vertex: bool,
    alpha_const: f32,
    blend: FxBlend,
}

/// Immutable result of the map-load FX promotion pass.  Keys use the same
/// stripped/lower-case effect namespace as `FxSystem::register`, with aliases
/// for both `foo/bar` and `effects/foo/bar` because game/mod code uses both.
#[derive(Clone, Debug, Default)]
pub struct FxLightPromotionCatalog {
    effects: HashMap<String, AutoLightProfile>,
    materials: HashMap<String, Option<MaterialLightAnalysis>>,
    scanned_effects: usize,
    semantic_candidates: usize,
    promotable_effects: usize,
    /// Prepared on map worker: one emissive driver per EFX for level-2
    /// projectile anchoring, with fallback when the semantic-first shader
    /// is absent or non-emissive. No VFS/material work on a render frame.
    projectile_drivers: HashMap<String, ProjectileLightDriver>,
}

impl FxLightPromotionCatalog {
    pub fn effect_profile(&self, key: &str) -> Option<AutoLightProfile> {
        self.effects.get(&effect_key(key)).copied()
    }

    fn material_analysis(&self, shader: &str) -> Option<MaterialLightAnalysis> {
        self.materials
            .get(&scene::canonical_shader_name(shader))
            .copied()
            .flatten()
    }

    pub fn scanned_effects(&self) -> usize {
        self.scanned_effects
    }

    pub fn semantic_candidates(&self) -> usize {
        self.semantic_candidates
    }

    pub fn promotable_effects(&self) -> usize {
        self.promotable_effects
    }

    pub fn analyzed_materials(&self) -> usize {
        self.materials.len()
    }
}

/// Indexed EFX qpaths in the active VFS.  Listing is cheap because
/// `AssetSearchPath` already owns the PK3/loose-file namespace index.
pub fn effect_asset_names(assets: &AssetSearchPath) -> Vec<String> {
    let mut names = assets
        .names()
        .filter(|name| {
            let lower = name.to_ascii_lowercase();
            lower.starts_with("effects/") && lower.ends_with(".efx")
        })
        .map(|name| name.replace('\\', "/"))
        .collect::<Vec<_>>();
    names.sort_unstable_by_key(|name| name.to_ascii_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    names
}

/// Build every promotion profile on a map worker.  `progress(done, total)` is
/// called after each EFX so the normal map-loading UI can expose the work.
pub fn build_catalog<F>(
    assets: &mut AssetSearchPath,
    shaders: &BTreeMap<String, Shader>,
    effect_names: &[String],
    mut progress: F,
) -> FxLightPromotionCatalog
where
    F: FnMut(u32, u32),
{
    let total = effect_names.len().min(u32::MAX as usize) as u32;
    let mut catalog = FxLightPromotionCatalog::default();
    let mut textures = Textures::new();

    for (index, qpath) in effect_names.iter().enumerate() {
        catalog.scanned_effects += 1;
        let relative = effect_relative_key(qpath);
        let parsed = assets
            .read(qpath, MAX_EFX_BYTES)
            .ok()
            .flatten()
            .map(|asset| template::parse_effect(&relative, &super::gp2::parse(&asset.bytes)));

        if let Some(template) = parsed {
            // The aggressive mode can use a different primitive from mode 1:
            // material evidence decides which available shader actually glows,
            // not just the semantic score. Negative smoke/debris classes remain
            // excluded even at the higher setting.
            if let Some(profile) = template.auto_light {
                let mut best: Option<(i16, ProjectileLightDriver)> = None;
                for primitive in &template.primitives {
                    if !matches!(primitive.kind,
                        PrimType::Particle | PrimType::OrientedParticle | PrimType::Line |
                        PrimType::Tail | PrimType::Cylinder | PrimType::Electricity)
                    { continue; }
                    for media in &primitive.media {
                        let semantic = template::token_score(media, false);
                        if semantic < 0 { continue; }
                        let key = scene::canonical_shader_name(media);
                        let analysis = if let Some(cached) = catalog.materials.get(&key) {
                            *cached
                        } else {
                            let analyzed = analyze_shader_material(assets, &mut textures, shaders, media);
                            catalog.materials.insert(key, analyzed);
                            analyzed
                        };
                        let Some(analysis) = analysis else { continue; };
                        let score = profile.score + semantic + analysis.score;
                        if score < AGGRESSIVE_PROMOTION_SCORE { continue; }
                        let midpoint = |min: f32, max: f32| 0.5 * (min + max);
                        let to_byte = |value: f32| (value.clamp(0.0, 1.0) * 255.0) as u8;
                        let rgba = [
                            to_byte(midpoint(primitive.rgb_start[0].min, primitive.rgb_start[0].max)),
                            to_byte(midpoint(primitive.rgb_start[1].min, primitive.rgb_start[1].max)),
                            to_byte(midpoint(primitive.rgb_start[2].min, primitive.rgb_start[2].max)),
                            to_byte(midpoint(primitive.alpha_start.min, primitive.alpha_start.max)),
                        ];
                        let radius = midpoint(primitive.size_start.min, primitive.size_start.max).abs()
                            .max(midpoint(primitive.size_end.min, primitive.size_end.max).abs())
                            .max(1.0);
                        let driver = ProjectileLightDriver {
                            kind: primitive.kind, shader: media.clone(), score: profile.score + semantic,
                            radius, rgba,
                        };
                        if best.as_ref().is_none_or(|(rank, _)| score > *rank) {
                            best = Some((score, driver));
                        }
                    }
                }
                if let Some((_, driver)) = best {
                    catalog.projectile_drivers.insert(relative.clone(), driver.clone());
                    catalog.projectile_drivers.insert(format!("effects/{relative}"), driver);
                }
            }
            if let Some(profile) = template.auto_light {
                catalog.semantic_candidates += 1;
                let driver = template.primitives.get(profile.driver_primitive);
                let mut final_candidate = false;
                if let Some(driver) = driver {
                    for shader in &driver.media {
                        let key = scene::canonical_shader_name(shader);
                        let analysis = if let Some(cached) = catalog.materials.get(&key) {
                            *cached
                        } else {
                            let analyzed = analyze_shader_material(
                                assets,
                                &mut textures,
                                shaders,
                                shader,
                            );
                            catalog.materials.insert(key, analyzed);
                            analyzed
                        };
                        if analysis.is_some_and(|analysis| {
                            profile.score + analysis.score >= FINAL_PROMOTION_SCORE
                        }) {
                            final_candidate = true;
                        }
                    }
                }

                // Keep semantic candidates even when their current material is
                // rejected. A later R_RemapShader can turn the same primitive
                // into an emissive stage, and the cold remap path intentionally
                // falls back to live material analysis.
                let prefixed = format!("effects/{relative}");
                catalog.effects.insert(relative, profile);
                catalog.effects.insert(prefixed, profile);
                if final_candidate {
                    catalog.promotable_effects += 1;
                }
            }
        }

        let done = (index + 1).min(u32::MAX as usize) as u32;
        progress(done, total);
    }

    // Direct CG_Disruptor/Concussion beam visuals bypass .efx entirely.
    // Prepare their stock materials once with the catalog on the map worker,
    // so aggressive beam promotion never decodes images on a render frame.
    for shader in ["gfx/effects/blueLine", "gfx/effects/redLine", "gfx/misc/whiteline2"] {
        let key = scene::canonical_shader_name(shader);
        if !catalog.materials.contains_key(&key) {
            let analyzed = analyze_shader_material(assets, &mut textures, shaders, shader);
            catalog.materials.insert(key, analyzed);
        }
    }

    catalog
}

fn effect_relative_key(path: &str) -> String {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    let stripped = normalized
        .strip_suffix(".efx")
        .unwrap_or(normalized.as_str());
    stripped
        .strip_prefix("effects/")
        .unwrap_or(stripped)
        .to_owned()
}

/// Resolve map fx_runner names using the same namespace as EFX registration.
pub fn runner_effect_asset_names(runners: &[scene::MapFxRunner]) -> Vec<String> {
    let mut names: Vec<_> = runners.iter()
        .map(|runner| format!("effects/{}.efx", effect_relative_key(&runner.effect)))
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

fn effect_key(path: &str) -> String {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    normalized
        .strip_suffix(".efx")
        .unwrap_or(normalized.as_str())
        .to_owned()
}

fn analyze_shader_material(
    assets: &mut AssetSearchPath,
    textures: &mut Textures,
    shaders: &BTreeMap<String, Shader>,
    shader_name: &str,
) -> Option<MaterialLightAnalysis> {
    let key = scene::canonical_shader_name(shader_name);
    if let Some(shader) = shaders.get(&key) {
        let mut best: Option<MaterialLightAnalysis> = None;
        for stage in shader.stages.iter().filter(|stage| {
            !stage.image.is_empty()
                && (!stage.image.starts_with('$')
                    || stage.image.eq_ignore_ascii_case("$whiteimage")
                    || stage.image.eq_ignore_ascii_case("$lightmap"))
        }) {
            let blend = FxBlend::from_blend_func(&stage.blend);
            let alpha_matters = matches!(blend, FxBlend::AddAlpha | FxBlend::Alpha);
            let texture_color = if stage.image.eq_ignore_ascii_case("$whiteimage")
                || stage.image.eq_ignore_ascii_case("$lightmap")
            {
                [1.0; 3]
            } else {
                let Some(texture_index) = textures.load(assets, &stage.image, stage.clamp) else {
                    continue;
                };
                let Some(color) =
                    texture_emissive_color(&textures.images[texture_index], alpha_matters)
                else {
                    continue;
                };
                color
            };
            let Some(analysis) = material_analysis(
                blend,
                stage.glow,
                texture_color,
                stage.rgb_gen.uses_vertex_color(),
                if stage.rgb_gen == RgbGen::Const {
                    stage.color.unwrap_or([1.0; 3])
                } else {
                    [1.0; 3]
                },
                matches!(stage.alpha_gen, AlphaGen::Vertex | AlphaGen::OneMinusVertex),
                stage.alpha.unwrap_or(1.0),
            ) else {
                continue;
            };
            if best.map_or(true, |current| analysis.score > current.score) {
                best = Some(analysis);
            }
        }
        return best;
    }

    // Faithful RE_RegisterShader implicit-image fallback: image-only FX use a
    // regular alpha blend when there is no .shader definition.
    let texture_index = textures.load(assets, &key, false)?;
    let texture_color = texture_emissive_color(&textures.images[texture_index], true)?;
    material_analysis(
        FxBlend::Alpha,
        false,
        texture_color,
        true,
        [1.0; 3],
        true,
        1.0,
    )
}

fn material_analysis(
    blend: FxBlend,
    authored_glow: bool,
    texture_color: [f32; 3],
    rgb_vertex: bool,
    rgb_const: [f32; 3],
    alpha_vertex: bool,
    alpha_const: f32,
) -> Option<MaterialLightAnalysis> {
    let mut score = match blend {
        FxBlend::Add => 6,
        FxBlend::AddAlpha => 5,
        FxBlend::Alpha => 0,
        FxBlend::Opaque => -4,
        FxBlend::DstColorAdd | FxBlend::Modulate | FxBlend::Modulate2x | FxBlend::Darken => -8,
    };
    if authored_glow {
        score += 6;
    }
    (score >= 0).then_some(MaterialLightAnalysis {
        score,
        texture_color,
        rgb_vertex,
        rgb_const,
        alpha_vertex,
        alpha_const,
        blend,
    })
}

/// Per-session runtime side.  Normally this is pure catalog lookup.  A dynamic
/// shader remap invalidates only the material side and switches to the existing
/// EntityPresenter resolver as a rare cold-path fallback until the next map
/// load installs a fresh catalog.
pub struct FxLightPromoter {
    catalog: Arc<FxLightPromotionCatalog>,
    runtime_materials: HashMap<String, Option<MaterialLightAnalysis>>,
    resolve_runtime_materials: bool,
    recent_short_lights: Vec<(u32, FxLight, i32)>,
    last_promotion_time: i32,
}

impl Default for FxLightPromoter {
    fn default() -> Self {
        Self {
            catalog: Arc::new(FxLightPromotionCatalog::default()),
            runtime_materials: HashMap::new(),
            resolve_runtime_materials: false,
            recent_short_lights: Vec::new(),
            last_promotion_time: i32::MIN,
        }
    }
}

impl FxLightPromoter {
    pub fn set_catalog(&mut self, catalog: Arc<FxLightPromotionCatalog>) {
        self.catalog = catalog;
        self.runtime_materials.clear();
        self.resolve_runtime_materials = false;
        self.recent_short_lights.clear();
    }

    pub fn invalidate_materials(&mut self) {
        self.runtime_materials.clear();
        self.resolve_runtime_materials = true;
    }

    /// Return a precomputed emissive emitter at the current missile position.
    /// The caller supplies the live interpolated origin, not an EFX sample
    /// that may be older than the last cg_fxFPS tick.
    pub fn projectile_candidate(&self, effect: &str, origin: [f32; 3]) -> Option<FxAutoLightCandidate> {
        let key = effect_key(effect);
        let spec = self.catalog.projectile_drivers.get(&key)?;
        // Deterministic ID groups identical effects for existing spatial merge.
        let hash = key.bytes().fold(2166136261_u32, |h, b| h.wrapping_mul(16777619) ^ u32::from(b));
        Some(FxAutoLightCandidate {
            effect: hash, score: spec.score, kind: spec.kind, origin,
            radius: spec.radius, rgba: spec.rgba, shader: spec.shader.clone(),
            life_ms: 1000, spot: None, segment: None,
        })
    }

    pub fn promote(
        &mut self,
        candidates: &[FxAutoLightCandidate],
        materials: &mut dyn FnMut(&str) -> Vec<FxMaterial>,
        time: i32,
        level: u8,
    ) -> Vec<FxLight> {
        struct Pending {
            effect: u32,
            light: FxLight,
        }

        if level == 0 { self.recent_short_lights.clear(); return Vec::new(); }
        if time < self.last_promotion_time { self.recent_short_lights.clear(); }
        self.last_promotion_time = time;
        if level != 2 { self.recent_short_lights.clear(); }
        self.recent_short_lights.retain(|(_, _, until)| *until >= time);
        let threshold = if level == 2 { AGGRESSIVE_PROMOTION_SCORE } else { FINAL_PROMOTION_SCORE };
        let mut pending: Vec<Pending> = Vec::new();
        for candidate in candidates {
            let key = scene::canonical_shader_name(&candidate.shader);
            let analysis = if self.resolve_runtime_materials {
                if let Some(cached) = self.runtime_materials.get(&key) {
                    *cached
                } else {
                    let resolved = materials(&candidate.shader);
                    let analyzed = analyze_materials(&resolved);
                    self.runtime_materials.insert(key, analyzed);
                    analyzed
                }
            } else {
                self.catalog.material_analysis(&candidate.shader)
            };
            let Some(analysis) = analysis else { continue };
            if candidate.score + analysis.score < threshold {
                continue;
            }

            let mut color = analysis.texture_color;
            let vertex_rgb = [
                f32::from(candidate.rgba[0]) / 255.0,
                f32::from(candidate.rgba[1]) / 255.0,
                f32::from(candidate.rgba[2]) / 255.0,
            ];
            let tint = if analysis.rgb_vertex {
                vertex_rgb
            } else {
                analysis.rgb_const
            };
            for channel in 0..3 {
                color[channel] *= tint[channel].max(0.0);
            }
            let alpha = if analysis.alpha_vertex {
                f32::from(candidate.rgba[3]) / 255.0
            } else {
                analysis.alpha_const
            };
            let alpha_weight = match analysis.blend {
                FxBlend::AddAlpha | FxBlend::Alpha => alpha.clamp(0.0, 1.0),
                _ => 1.0,
            };
            for channel in &mut color {
                *channel *= alpha_weight;
            }
            let peak = color[0].max(color[1]).max(color[2]);
            let minimum_peak = if level == 2 { 0.015 } else { 0.03 };
            if !peak.is_finite() || peak < minimum_peak {
                continue;
            }

            // Keep chromaticity from the source texture/vertex tint but avoid
            // treating a dim texture average as a second arbitrary intensity
            // control. Alpha/fades still modulate the light.
            let chroma_peak = peak.max(0.20);
            color = color.map(|value| (value / chroma_peak).clamp(0.0, 1.0));

            let mut origin = candidate.origin;
            let mut spot = candidate.spot;
            let mut segment = candidate.segment;
            if let Some([start, end]) = candidate.segment {
                origin = std::array::from_fn(|i| 0.5 * (start[i] + end[i]));
                // Level 2 can treat truly long straight FX lines as a projector;
                // short blaster trails remain segment emitters. The renderer
                // already supports spots, so this adds no new shading path.
                let direction: [f32; 3] = std::array::from_fn(|i| end[i] - start[i]);
                let length = distance_squared(start, end).sqrt();
                if level == 2 && candidate.kind == PrimType::Line && length >= 384.0 {
                    let cone = (candidate.radius.abs().max(6.0) / length).atan().clamp(0.10, 0.45);
                    spot = Some(super::system::FxSpotLight {
                        direction: direction.map(|v| v / length),
                        inner_cone_cos: (cone * 0.65).cos(), outer_cone_cos: cone.cos(),
                    });
                    segment = None;
                    origin = start;
                }
            }
            let radius = inferred_radius(candidate);
            let source_radius = inferred_source_radius(candidate, radius);
            let light = FxLight {
                kind: FxLightKind::PromotedEffect,
                origin,
                radius,
                source_radius,
                rgb: color,
                spot,
                segment,
                blade_segments: None,
            };

            // Small EFX lifetimes (e.g. 10ms) are shorter than cg_fxFPS ticks.
            // Extend LIGHT ONLY, never the primitive's authored draw/lifetime.
            if level == 2 && candidate.life_ms > 0 && candidate.life_ms < PROMOTED_LIGHT_MIN_LIFE_MS {
                let until = time.saturating_add(PROMOTED_LIGHT_MIN_LIFE_MS - candidate.life_ms);
                if let Some(held) = self.recent_short_lights.iter_mut().find(|(effect, old, _)| {
                    *effect == candidate.effect && distance_squared(old.origin, light.origin) <= 48.0 * 48.0
                }) {
                    held.1 = light.clone();
                    held.2 = until;
                } else if self.recent_short_lights.len() < MAX_PROMOTED_LIGHTS {
                    self.recent_short_lights.push((candidate.effect, light.clone(), until));
                }
            }

            // Continuous JKA effects may be invoked repeatedly while an older
            // driver primitive is still alive. Merge only same-effect lights
            // that are spatially close, preserving distinct explosions/bolts.
            let merge_distance = (radius * 0.35).clamp(24.0, 96.0);
            if let Some(existing) = pending.iter_mut().find(|existing| {
                let directional_match = match (existing.light.spot, light.spot) {
                    (Some(a), Some(b)) => {
                        a.direction[0] * b.direction[0]
                            + a.direction[1] * b.direction[1]
                            + a.direction[2] * b.direction[2]
                            >= 0.94
                    }
                    (None, None) => true,
                    _ => false,
                };
                existing.effect == candidate.effect
                    && directional_match
                    && distance_squared(existing.light.origin, light.origin)
                        <= merge_distance * merge_distance
            }) {
                existing.light.radius = existing.light.radius.max(light.radius);
                existing.light.source_radius = existing.light.source_radius.max(light.source_radius);
                for i in 0..3 {
                    existing.light.rgb[i] = existing.light.rgb[i].max(light.rgb[i]);
                }
                if existing.light.segment.is_none() {
                    existing.light.segment = light.segment;
                }
                match (&mut existing.light.spot, light.spot) {
                    (Some(current), Some(incoming)) => {
                        let summed = [
                            current.direction[0] + incoming.direction[0],
                            current.direction[1] + incoming.direction[1],
                            current.direction[2] + incoming.direction[2],
                        ];
                        let length_sq = summed.iter().map(|v| v * v).sum::<f32>();
                        if length_sq > 1.0e-6 {
                            let inv = length_sq.sqrt().recip();
                            current.direction = summed.map(|v| v * inv);
                        }
                        // Smaller cosine means a wider cone. Keep the wider
                        // authored/inferred envelope when repeated driver
                        // primitives are merged into one effect light.
                        current.inner_cone_cos = current.inner_cone_cos.min(incoming.inner_cone_cos);
                        current.outer_cone_cos = current.outer_cone_cos.min(incoming.outer_cone_cos);
                    }
                    (slot @ None, Some(incoming)) => *slot = Some(incoming),
                    _ => {}
                }
            } else if pending.len() < MAX_PROMOTED_LIGHTS {
                pending.push(Pending {
                    effect: candidate.effect,
                    light,
                });
            }
        }
        // Keep recently expired short-lived promoted emitters for the minimum
        // light-only lifespan. Avoid doubling a currently submitted light.
        if level == 2 {
            for (effect, light, _) in &self.recent_short_lights {
                if pending.len() >= MAX_PROMOTED_LIGHTS { break; }
                if !pending.iter().any(|present| present.effect == *effect
                    && distance_squared(present.light.origin, light.origin) <= 48.0 * 48.0)
                {
                    pending.push(Pending { effect: *effect, light: light.clone() });
                }
            }
        }
        pending.into_iter().map(|pending| pending.light).collect()
    }
}

fn analyze_materials(materials: &[FxMaterial]) -> Option<MaterialLightAnalysis> {
    materials
        .iter()
        .filter_map(|material| {
            let alpha_matters = matches!(material.blend, FxBlend::AddAlpha | FxBlend::Alpha);
            let texture_color = match material.texture.as_deref() {
                Some(texture) => texture_emissive_color(texture, alpha_matters)?,
                None => [1.0; 3],
            };
            material_analysis(
                material.blend,
                material.authored_glow,
                texture_color,
                material.rgb_vertex,
                material.rgb_const,
                material.alpha_vertex,
                material.alpha_const,
            )
        })
        .max_by_key(|analysis| analysis.score)
}

fn texture_emissive_color(
    texture: &crate::materials::TextureData,
    alpha_matters: bool,
) -> Option<[f32; 3]> {
    if texture.label.eq_ignore_ascii_case("missing texture fallback") || texture.rgba.len() < 4 {
        return None;
    }
    // TextureData may contain appended mip levels. Sample only mip 0 so a
    // long mip chain cannot bias the inferred colour toward tiny levels.
    let base_bytes = (texture.width as usize)
        .saturating_mul(texture.height as usize)
        .saturating_mul(4)
        .min(texture.rgba.len());
    let pixels = base_bytes / 4;
    if pixels == 0 {
        return None;
    }
    let stride = (pixels / MAX_TEXTURE_SAMPLES).max(1);
    let mut sum = [0.0_f64; 3];
    let mut total = 0.0_f64;
    for pixel in (0..pixels).step_by(stride).take(MAX_TEXTURE_SAMPLES) {
        let base = pixel * 4;
        // Plain additive blending ignores source alpha. Respect alpha only
        // for blend modes where it actually modulates rendered energy.
        let alpha = if alpha_matters {
            f64::from(texture.rgba[base + 3]) / 255.0
        } else {
            1.0
        };
        if alpha <= 0.02 {
            continue;
        }
        let mut rgb = [0.0_f64; 3];
        for channel in 0..3 {
            let encoded = f64::from(texture.rgba[base + channel]) / 255.0;
            rgb[channel] = if texture.srgb {
                srgb_to_linear(encoded)
            } else {
                encoded
            };
        }
        let luminance = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
        // Squared luminance makes bright cores dominate black/transparent halo
        // backgrounds without hard-coded knowledge of individual textures.
        let weight = alpha * luminance * luminance;
        if weight <= 1.0e-6 {
            continue;
        }
        for channel in 0..3 {
            sum[channel] += rgb[channel] * weight;
        }
        total += weight;
    }
    if total <= 1.0e-6 {
        return None;
    }
    let mut color = sum.map(|value| (value / total) as f32);
    let peak = color[0].max(color[1]).max(color[2]);
    if peak <= 1.0e-5 || !peak.is_finite() {
        return None;
    }
    color = color.map(|value| (value / peak).clamp(0.0, 1.0));
    Some(color)
}

fn srgb_to_linear(value: f64) -> f64 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn inferred_radius(candidate: &FxAutoLightCandidate) -> f32 {
    let visual = candidate.radius.abs().max(1.0);
    let mut radius = match candidate.kind {
        PrimType::Electricity => 96.0 + visual * 14.0,
        PrimType::Line | PrimType::Tail => 72.0 + visual * 12.0,
        PrimType::Cylinder => 72.0 + visual * 10.0,
        _ => 56.0 + visual * 8.0,
    };
    if let Some([start, end]) = candidate.segment {
        let length = distance_squared(start, end).sqrt();
        radius = radius.max(48.0 + length.min(512.0) * 0.25);
    }
    radius.clamp(48.0, 384.0)
}

/// Estimate the luminous visual's physical size independently from its much
/// larger attenuation radius. RT shadows use this for penumbrae; raster modes
/// deliberately ignore it.
fn inferred_source_radius(candidate: &FxAutoLightCandidate, illumination_radius: f32) -> f32 {
    let visual = candidate.radius.abs().max(0.0);
    let source = match candidate.kind {
        // These already become finite line emitters in RT; this adds thickness,
        // effectively turning the line into a small capsule emitter.
        PrimType::Electricity | PrimType::Line | PrimType::Tail => visual * 0.45,
        // A tapered Cylinder promoted as a spot is a lens/cone: its visible
        // far ring is not the physical source aperture.
        PrimType::Cylinder if candidate.spot.is_some() => visual * 0.15,
        PrimType::Cylinder => visual * 0.50,
        // Fire/flash/explosion billboard drivers are reasonably approximated by
        // a fuzzy spherical source around roughly half their visible radius.
        PrimType::Particle | PrimType::OrientedParticle => visual * 0.60,
        _ => visual * 0.50,
    };
    source
        .clamp(1.5, 64.0)
        .min((illumination_radius * 0.25).max(1.5))
}

fn distance_squared(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]) * (a[0] - b[0])
        + (a[1] - b[1]) * (a[1] - b[1])
        + (a[2] - b[2]) * (a[2] - b[2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::materials::TextureData;

    #[test]
    fn bright_texture_color_ignores_black_background() {
        let texture = TextureData {
            label: "test".into(),
            source: None,
            width: 2,
            height: 1,
            rgba: vec![0, 0, 0, 255, 255, 64, 0, 255],
            rgba16f: None,
            mip_level_count: 1,
            clamp: false,
            srgb: true,
        };
        let color = texture_emissive_color(&texture, true).unwrap();
        assert!(color[0] > 0.95);
        assert!(color[1] < 0.2);
        assert!(color[2] < 0.05);
    }

    #[test]
    fn effect_catalog_key_accepts_prefixed_and_unprefixed_names() {
        let mut catalog = FxLightPromotionCatalog::default();
        let profile = AutoLightProfile {
            driver_primitive: 2,
            score: 9,
            spot: None,
        };
        catalog.effects.insert("foo/bar".into(), profile);
        catalog.effects.insert("effects/foo/bar".into(), profile);
        assert_eq!(catalog.effect_profile("foo/bar.efx"), Some(profile));
        assert_eq!(catalog.effect_profile("effects/foo/bar"), Some(profile));
    }
}

#[cfg(test)]
mod promotion_level_tests {
    use super::*;

    fn candidate() -> FxAutoLightCandidate {
        FxAutoLightCandidate { effect: 7, score: 0, kind: PrimType::Particle,
            origin: [1.0, 2.0, 3.0], radius: 4.0, rgba: [255; 4],
            shader: "gfx/effects/testglow".to_owned(), life_ms: 10,
            spot: None, segment: None }
    }

    #[test]
    fn aggressive_is_less_restrictive_without_changing_conservative() {
        let mut catalog = FxLightPromotionCatalog::default();
        catalog.materials.insert(scene::canonical_shader_name("gfx/effects/testglow"),
            Some(MaterialLightAnalysis { score: 6, texture_color: [1.0; 3],
                rgb_vertex: true, rgb_const: [1.0; 3], alpha_vertex: false,
                alpha_const: 1.0, blend: FxBlend::Add }));
        let mut promoter = FxLightPromoter::default();
        promoter.set_catalog(Arc::new(catalog));
        let mut no_materials = |_: &str| Vec::new();
        assert!(promoter.promote(&[candidate()], &mut no_materials, 1000, 1).is_empty());
        assert_eq!(promoter.promote(&[candidate()], &mut no_materials, 1000, 2).len(), 1);
    }

    #[test]
    fn minimum_light_life_does_not_extend_visual_primitives() {
        let mut catalog = FxLightPromotionCatalog::default();
        catalog.materials.insert(scene::canonical_shader_name("gfx/effects/testglow"),
            Some(MaterialLightAnalysis { score: 6, texture_color: [1.0; 3],
                rgb_vertex: true, rgb_const: [1.0; 3], alpha_vertex: false,
                alpha_const: 1.0, blend: FxBlend::Add }));
        let mut promoter = FxLightPromoter::default();
        promoter.set_catalog(Arc::new(catalog));
        let mut no_materials = |_: &str| Vec::new();
        assert_eq!(promoter.promote(&[candidate()], &mut no_materials, 1000, 2).len(), 1);
        assert_eq!(promoter.promote(&[], &mut no_materials, 1020, 2).len(), 1);
        assert!(promoter.promote(&[], &mut no_materials, 1031, 2).is_empty());
    }
}
