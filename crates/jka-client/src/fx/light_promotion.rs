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
// Shared, material-independent parameters for EVERY aggressive FX_AddLine
// projector. Preserve per-line RGB/alpha, but don't let normalized emissive
// colour create a full-bright floodlight with a long beam falloff range.
const AGGRESSIVE_LINE_STRENGTH: f32 = 0.25;
const AGGRESSIVE_LINE_MIN_OUTER_HALF_ANGLE_DEG: f32 = 6.0;
const AGGRESSIVE_LINE_MAX_OUTER_HALF_ANGLE_DEG: f32 = 12.0;

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
    static_emission: bool,
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
    fixed_effects: HashMap<String, (scene::DynamicLight, f32)>,
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

    /// Only untargeted, continuously active fixtures with a stable point emitter
    /// are eligible. No projectile, short pulse, spot or moving particle is baked.
    pub fn static_runner_lights(&self, runners: &[scene::MapFxRunner]) -> Vec<scene::DynamicLight> {
        runners.iter().filter_map(|runner| {
            if !runner.bake_static || runner.spawnflags & 3 != 0 || runner.random_ms != 0 { return None; }
            let (mut light,life)=*self.fixed_effects.get(&effect_relative_key(&runner.effect))?;
            if life < runner.delay_ms.max(1) as f32 { return None; }
            light.position=scene::render_position(runner.origin);
            Some(light)
        }).collect()
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

/// Every material key the serial catalog walk analyzes for `template`, in the
/// order it reaches them: aggressive-driver candidates, then the semantic
/// driver's media. `fixed_light_profile` only reads the driver's media, which
/// this includes, so a catalog prefilled with these keys is identical.
fn template_material_names<'a>(template: &'a template::EffectTemplate, out: &mut Vec<&'a str>) {
    let Some(profile) = template.auto_light else { return };
    for primitive in &template.primitives {
        if !matches!(primitive.kind,
            PrimType::Particle | PrimType::OrientedParticle | PrimType::Line |
            PrimType::Tail | PrimType::Cylinder | PrimType::Electricity)
        { continue; }
        for media in &primitive.media {
            if template::token_score(media, false) >= 0 {
                out.push(media);
            }
        }
    }
    if let Some(driver) = template.primitives.get(profile.driver_primitive) {
        out.extend(driver.media.iter().map(String::as_str));
    }
}

/// Build every promotion profile on a map worker.  `progress(done, total)`
/// reports the EFX parse (first half) and material analysis (second half) so
/// the normal map-loading UI can expose the work.
///
/// EFX reads/parses and material analyses (texture decode + sampling) are
/// pure per item, so both fan out over rayon with per-thread VFS forks; the
/// catalog is then assembled serially in effect order with every material
/// already cached, giving the same result as the one-thread walk.
pub fn build_catalog<F>(
    assets: &mut AssetSearchPath,
    shaders: &BTreeMap<String, Shader>,
    effect_names: &[String],
    progress: F,
) -> FxLightPromotionCatalog
where
    F: FnMut(u32, u32) + Send,
{
    use rayon::prelude::*;
    use std::sync::{
        atomic::{AtomicU32, Ordering},
        Mutex, PoisonError,
    };

    let total = effect_names.len().min(u32::MAX as usize) as u32;
    let half = total / 2;
    let progress = Mutex::new(progress);
    let report = |done: u32| {
        let mut progress = progress.lock().unwrap_or_else(PoisonError::into_inner);
        (*progress)(done.min(total), total);
    };
    let reader = assets.fork();

    let parsed_done = AtomicU32::new(0);
    let parsed: Vec<(String, Option<template::EffectTemplate>)> = effect_names
        .par_iter()
        .map_init(
            || reader.fork(),
            |reader, qpath| {
                let relative = effect_relative_key(qpath);
                let parsed = reader
                    .read(qpath, MAX_EFX_BYTES)
                    .ok()
                    .flatten()
                    .map(|asset| template::parse_effect(&relative, &super::gp2::parse(&asset.bytes)));
                let done = parsed_done.fetch_add(1, Ordering::Relaxed) + 1;
                if total > 0 {
                    report((u64::from(done) * u64::from(half) / u64::from(total)) as u32);
                }
                (relative, parsed)
            },
        )
        .collect();

    // Unique material keys, first-seen name kept (analysis depends only on
    // the canonical key), plus the stock beam shaders analyzed at the end.
    let mut seen = std::collections::HashSet::new();
    let mut material_names = Vec::new();
    let mut names = Vec::new();
    for template in parsed.iter().filter_map(|(_, template)| template.as_ref()) {
        names.clear();
        template_material_names(template, &mut names);
        for name in names.drain(..) {
            if seen.insert(scene::canonical_shader_name(name)) {
                material_names.push(name);
            }
        }
    }
    for name in ["gfx/effects/blueLine", "gfx/effects/redLine", "gfx/misc/whiteline2"] {
        if seen.insert(scene::canonical_shader_name(name)) {
            material_names.push(name);
        }
    }

    let material_total = material_names.len().max(1) as u64;
    let analyzed_done = AtomicU32::new(0);
    let analyzed: Vec<(String, Option<MaterialLightAnalysis>)> = material_names
        .par_iter()
        .map_init(
            || (reader.fork(), Textures::new()),
            |(reader, textures), name| {
                let analysis = analyze_shader_material(reader, textures, shaders, name);
                let done = u64::from(analyzed_done.fetch_add(1, Ordering::Relaxed) + 1);
                report(half + (done * u64::from(total - half) / material_total) as u32);
                (scene::canonical_shader_name(name), analysis)
            },
        )
        .collect();

    let mut catalog = FxLightPromotionCatalog::default();
    catalog.materials.extend(analyzed);
    let mut textures = Textures::new();

    for (relative, parsed) in parsed {
        catalog.scanned_effects += 1;
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
                catalog.effects.insert(relative.clone(), profile);
                catalog.effects.insert(prefixed, profile);
                if final_candidate {
                    catalog.promotable_effects += 1;
                }
            }
            if let Some(fixed) = fixed_light_profile(&template,&catalog) {
                catalog.fixed_effects.insert(relative, fixed);
            }
        }
    }
    report(total);

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
            let Some(mut analysis) = material_analysis(
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
            analysis.static_emission=stage.animation.is_none() && !stage.video && !stage.alpha_portal
                && !matches!(stage.rgb_gen,RgbGen::Wave(_)|RgbGen::LightingDiffuse|RgbGen::LightingDiffuseEntity)
                && !matches!(stage.alpha_gen,AlphaGen::Wave(_)|AlphaGen::LightingSpecular)
                && matches!(stage.tc_gen,jka_assets::shader::TcGen::Base|jka_assets::shader::TcGen::Vector(..))
                && stage.tc_mods.iter().all(|m|matches!(m,jka_assets::shader::TcMod::Scale(..)|jka_assets::shader::TcMod::Transform(..)));
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

/// Fallback for stock, code-constructed beam lines only; never an arbitrary
/// unknown or deliberately non-emissive EFX material. Their shaders are known
/// from TaystJK fx_disruptor.c and fx_concussion.c.
fn stock_beam_fallback(key: &str) -> Option<MaterialLightAnalysis> {
    let texture_color = match key {
        "gfx/effects/redline" => [1.0, 0.10, 0.04],
        "gfx/effects/blueline" => [0.25, 0.4, 1.0],
        "gfx/misc/whiteline2" => [1.0; 3],
        _ => return None,
    };
    material_analysis(FxBlend::Add, false, texture_color, true, [1.0; 3], false, 1.0)
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
        static_emission: true,
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
            // Retain actual line endpoints for *identity*, even though the
            // GPU light packs this as a spotlight with segment=None.
            line: Option<[[f32; 3]; 2]>,
        }

        let diag_start = crate::fx::diagnostics::enabled().then(std::time::Instant::now);
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
                    self.runtime_materials.insert(key.clone(), analyzed);
                    analyzed
                }
            } else {
                self.catalog.material_analysis(&candidate.shader)
            };
            // The direct FX_AddLine weapons are generated by CG_Disruptor and
            // CG_Concussion, not by any EFX template. The original game always
            // draws these named beam materials even when a PK3 lacks a separate
            // .shader definition. If map-load image analysis cannot resolve
            // that stock media, an aggressive beam still deserves its light.
            // Non-line FX keep their existing material requirements. Only
            // aggressive straight lines bypass shader evidence as requested.
            // Aggressive means *every* actual FX_AddLine. Don't require a
            // semantic driver, decoded texture, or emissive shader classification
            // before giving a displayed line its forward-facing projector.
            // Conservative mode retains all of its original evidence gates.
            let aggressive_line = level == 2 && candidate.kind == PrimType::Line
                && line_projector(candidate.segment, candidate.radius).is_some();
            let analysis = if aggressive_line {
                analysis.or_else(|| stock_beam_fallback(&key)).or_else(|| {
                    material_analysis(FxBlend::Add, false, [1.0; 3], true,
                        [1.0; 3], false, 1.0)
                })
            } else {
                analysis
            };
            let Some(analysis) = analysis else { continue };
            if !aggressive_line && candidate.score + analysis.score < threshold {
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
            // Affect ALL aggressive FX_AddLine lights, not named weapons,
            // individual shaders or FX file paths. Keep per-line hue and fade.
            if aggressive_line {
                color = color.map(|value| value * AGGRESSIVE_LINE_STRENGTH);
            }

            let mut origin = candidate.origin;
            let mut spot = candidate.spot;
            let mut segment = candidate.segment;
            let mut radius = inferred_radius(candidate);
            if let Some([start, end]) = candidate.segment {
                if level == 2 && candidate.kind == PrimType::Line {
                    if let Some((projector, length)) = line_projector(candidate.segment, candidate.radius) {
                        // FX_AddLine(start, end) points outward from the
                        // weapon muzzle. Never substitute a midpoint tube or
                        // aim the cone from the impact point. Every valid line
                        // gets a spotlight, including very short lines.
                        origin = start;
                        spot = Some(projector);
                        // Packing a segment would overwrite GPU spotlight.w=3.
                        segment = None;
                        // Beam lights must illuminate the target AND the
                        // nearby scene, instead of using a ~100u point radius.
                        radius = radius.max(length + 64.0).min(16_384.0);
                    }
                } else {
                    // Preserve non-aggressive segment handling unchanged.
                    origin = std::array::from_fn(|i| 0.5 * (start[i] + end[i]));
                }
            }
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

            // Aggressive line lights are a *projection of visible geometry*.
            // A persistent EFX may retain several historical submissions of
            // its same line (plus parallel core/glow layers). Those are ONE
            // physical beam and should use one spotlight, not fill 64 slots.
            // Check both endpoints: same muzzle alone must never merge
            // independent disruptor bolts, split beams or different targets.
            let merge_distance = (radius * 0.35).clamp(24.0, 96.0);
            if let Some(existing) = pending.iter_mut().find(|existing| {
                if aggressive_line {
                    return match (existing.line, candidate.segment) {
                        (Some(a), Some(b)) => same_visible_line(a, b),
                        _ => false,
                    };
                }
                if existing.line.is_some() { return false; }
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
                    line: if aggressive_line { candidate.segment } else { None },
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
                    pending.push(Pending { effect: *effect, light: light.clone(), line: None });
                }
            }
        }
        let result: Vec<FxLight> = pending.into_iter().map(|pending| pending.light).collect();
        if let Some(started) = diag_start {
            use crate::fx::diagnostics as diag;
            diag::add(diag::PROMOTION_CALLS, 1);
            diag::add(diag::PROMOTION_US, diag::micros(started.elapsed()));
            diag::add(diag::CANDIDATES, candidates.len() as u64);
            diag::peak(diag::MAX_CANDIDATES_FRAME, candidates.len() as u64);
            diag::add(diag::CANDIDATE_LINES,
                candidates.iter().filter(|c| c.kind == PrimType::Line).count() as u64);
            diag::add(diag::PROMOTED, result.len() as u64);
            diag::peak(diag::MAX_PROMOTED_FRAME, result.len() as u64);
            diag::add(diag::SPOTS, result.iter().filter(|l| l.spot.is_some()).count() as u64);
            for light in &result {
                if light.spot.is_some() && light.radius.is_finite() {
                    diag::peak(diag::MAX_SPOT_RADIUS_10, (light.radius.max(0.0) * 10.0) as u64);
                }
            }
            for candidate in candidates {
                if let Some([start, end]) = candidate.segment {
                    let distance = start.iter().zip(end).map(|(a, b)| (a-b)*(a-b)).sum::<f32>().sqrt();
                    if distance.is_finite() {
                        diag::peak(diag::MAX_BEAM_LENGTH_10, (distance.max(0.0) * 10.0) as u64);
                    }
                }
            }
        }
        result
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

/// Treat only the same geometric line as one projected light. Multiple
/// historical instances and authored glow/core overlays can share it, but
/// nearby parallel lines or beams aimed at different endpoints cannot.
fn same_visible_line(a: [[f32; 3]; 2], b: [[f32; 3]; 2]) -> bool {
    // Small epsilon tolerates FX float interpolation without merging adjacent
    // laser paths or different impact points.
    const ENDPOINT_EPS_SQ: f32 = 1.0;
    distance_squared(a[0], b[0]) <= ENDPOINT_EPS_SQ
        && distance_squared(a[1], b[1]) <= ENDPOINT_EPS_SQ
}

/// One forward-facing muzzle/start spotlight per valid aggressive FX_AddLine.
/// A beam needs a small projected cone, not a 70-degree floodlight. Use the
/// actual visible line width to widen the half-angle slightly (within narrow
/// bounds), independent of effect/weapon/shader names or beam travel distance.
fn line_projector(
    segment: Option<[[f32; 3]; 2]>,
    visible_width: f32,
) -> Option<(super::system::FxSpotLight, f32)> {
    let [start, end] = segment?;
    if !start.iter().chain(end.iter()).all(|value| value.is_finite()) { return None; }
    let delta = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
    let length_squared = delta.iter().map(|v| v * v).sum::<f32>();
    if !length_squared.is_finite() || length_squared <= 1.0e-8 { return None; }
    let length = length_squared.sqrt();
    let width = if visible_width.is_finite() {
        visible_width.abs().min(32.0)
    } else {
        0.0
    };
    let outer_half_angle_deg = (AGGRESSIVE_LINE_MIN_OUTER_HALF_ANGLE_DEG + width * 0.35)
        .min(AGGRESSIVE_LINE_MAX_OUTER_HALF_ANGLE_DEG);
    let inner_half_angle_deg = outer_half_angle_deg * 0.60;
    Some((super::system::FxSpotLight {
        direction: delta.map(|v| v / length),
        inner_cone_cos: inner_half_angle_deg.to_radians().cos(),
        outer_cone_cos: outer_half_angle_deg.to_radians().cos(),
    }, length))
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
            rgba: (vec![0, 0, 0, 255, 255, 64, 0, 255]).into(),
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
            Some(MaterialLightAnalysis { static_emission: true, score: 6, texture_color: [1.0; 3],
                rgb_vertex: true, rgb_const: [1.0; 3], alpha_vertex: false,
                alpha_const: 1.0, blend: FxBlend::Add }));
        let mut promoter = FxLightPromoter::default();
        promoter.set_catalog(Arc::new(catalog));
        let mut no_materials = |_: &str| Vec::new();
        assert!(promoter.promote(&[candidate()], &mut no_materials, 1000, 1).is_empty());
        assert_eq!(promoter.promote(&[candidate()], &mut no_materials, 1000, 2).len(), 1);
    }

    #[test]
    fn stock_direct_beam_can_promote_without_catalog_texture() {
        let mut promoter = FxLightPromoter::default();
        let mut no_materials = |_: &str| Vec::new();
        let beam = FxAutoLightCandidate {
            effect: 42, score: 3, kind: PrimType::Line,
            origin: [0.0; 3], radius: 5.0, rgba: [255; 4],
            shader: "gfx/effects/redLine".into(), life_ms: 150,
            spot: None, segment: Some([[0.0; 3], [1000.0, 0.0, 0.0]]),
        };
        assert!(promoter.promote(&[beam.clone()], &mut no_materials, 1000, 1).is_empty());
        let lights = promoter.promote(&[beam], &mut no_materials, 1000, 2);
        assert_eq!(lights.len(), 1);
        assert!(lights[0].spot.is_some());
        assert!(lights[0].rgb[0] > lights[0].rgb[2]);
    }

    #[test]
    fn aggressive_long_disruptor_line_becomes_muzzle_spot_reaching_impact() {
        let mut catalog = FxLightPromotionCatalog::default();
        catalog.materials.insert(scene::canonical_shader_name("gfx/effects/redLine"),
            Some(MaterialLightAnalysis { static_emission: true, score: 6, texture_color: [1.0, 0.12, 0.04],
                rgb_vertex: true, rgb_const: [1.0; 3], alpha_vertex: false,
                alpha_const: 1.0, blend: FxBlend::Add }));
        let mut promoter = FxLightPromoter::default();
        promoter.set_catalog(Arc::new(catalog));
        let beam = FxAutoLightCandidate {
            effect: 19, score: 3, kind: PrimType::Line,
            origin: [0.0; 3], radius: 6.0, rgba: [255; 4],
            shader: "gfx/effects/redLine".into(), life_ms: 150,
            spot: None, segment: Some([[0.0, 0.0, 0.0], [1200.0, 0.0, 0.0]]),
        };
        let mut no_materials = |_: &str| Vec::new();
        let lights = promoter.promote(&[beam.clone()], &mut no_materials, 1000, 2);
        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0].origin, [0.0; 3]);
        assert!(lights[0].spot.is_some());
        assert_eq!(lights[0].segment, None);
        assert!(lights[0].radius >= 1200.0);
        let conservative = promoter.promote(&[beam], &mut no_materials, 1000, 1);
        assert_eq!(conservative.len(), 1);
        assert!(conservative[0].spot.is_none());
        assert!(conservative[0].segment.is_some());
    }

    #[test]
    fn aggressive_projects_every_line_from_muzzle_even_without_shader_evidence() {
        let mut promoter = FxLightPromoter::default();
        let mut no_materials = |_: &str| Vec::new();
        let beam = FxAutoLightCandidate {
            effect: 10, score: -1000, kind: PrimType::Line,
            origin: [123.0; 3], radius: 1.0, rgba: [255, 100, 50, 255],
            shader: "custom/unclassified_line".into(), life_ms: 150,
            spot: None, segment: Some([[2.0, 3.0, 4.0], [1202.0, 3.0, 4.0]]),
        };
        assert!(promoter.promote(&[beam.clone()], &mut no_materials, 1000, 1).is_empty());
        let promoted = promoter.promote(&[beam.clone()], &mut no_materials, 1000, 2);
        assert_eq!(promoted.len(), 1);
        assert_eq!(promoted[0].origin, [2.0, 3.0, 4.0]);
        assert_eq!(promoted[0].segment, None);
        assert_eq!(promoted[0].spot.unwrap().direction, [1.0, 0.0, 0.0]);
        assert!(promoted[0].radius >= 1200.0);
        assert!(promoted[0].spot.unwrap().outer_cone_cos > 0.97);

        // Identical historical submissions (and core/glow .efx layers)
        // represent the SAME visible beam, hence only one physical spotlight.
        let two = promoter.promote(&[beam.clone(), beam.clone()], &mut no_materials, 1005, 2);
        assert_eq!(two.len(), 1);
        assert!(two[0].spot.is_some());

        let mut core = beam.clone();
        core.effect = 999; // Different effect IDs must not prevent dedup.
        core.shader = "different/hot_core".into();
        core.radius = 0.2;
        assert_eq!(promoter.promote(&[beam.clone(), core], &mut no_materials, 1005, 2).len(), 1);

        // Nearby but genuinely different lines still get their own spotlights.
        let mut parallel = beam.clone();
        parallel.segment = Some([[2.0, 5.0, 4.0], [1202.0, 5.0, 4.0]]);
        let mut other_target = beam.clone();
        other_target.segment = Some([[2.0, 3.0, 4.0], [1202.0, 12.0, 4.0]]);
        assert_eq!(promoter.promote(&[beam.clone(), parallel, other_target], &mut no_materials, 1005, 2).len(), 3);

        // Even a short line is a projector: the old 384u threshold is gone.
        let mut short = beam.clone();
        short.segment = Some([[2.0, 3.0, 4.0], [12.0, 3.0, 4.0]]);
        assert!(promoter.promote(&[short], &mut no_materials, 1010, 2)[0].spot.is_some());
    }

    #[test]
    fn aggressive_line_tuning_is_generic_and_preserves_per_line_colour() {
        let mut promoter = FxLightPromoter::default();
        let mut no_materials = |_: &str| Vec::new();
        let beam = FxAutoLightCandidate {
            effect: 80, score: -100, kind: PrimType::Line,
            origin: [99.0; 3], radius: 2.0, rgba: [200, 100, 50, 255],
            shader: "mods/beam_a".into(), life_ms: 150,
            spot: None, segment: Some([[0.0; 3], [1000.0, 0.0, 0.0]]),
        };
        let light_a = promoter.promote(&[beam.clone()], &mut no_materials, 1000, 2).remove(0);
        let mut alternate = beam.clone();
        alternate.shader = "mods/beam_b".into();
        let light_b = promoter.promote(&[alternate], &mut no_materials, 1010, 2).remove(0);
        assert_eq!(light_a.rgb, light_b.rgb);
        assert!((light_a.rgb[0] - AGGRESSIVE_LINE_STRENGTH).abs() < 1.0e-5);
        assert!(light_a.rgb[0] > light_a.rgb[1] && light_a.rgb[1] > light_a.rgb[2]);
        assert_eq!(light_a.spot, light_b.spot);
        assert_eq!(light_a.origin, [0.0; 3]);
        let narrow = line_projector(beam.segment, 0.1).unwrap().0;
        let thick = line_projector(beam.segment, 32.0).unwrap().0;
        assert!(thick.outer_cone_cos < narrow.outer_cone_cos);
        assert!(thick.outer_cone_cos >= AGGRESSIVE_LINE_MAX_OUTER_HALF_ANGLE_DEG.to_radians().cos());
        assert!(narrow.inner_cone_cos > narrow.outer_cone_cos);
        // The aggressive tuning never changes Conservative line promotion.
        let conservative = promoter.promote(&[beam], &mut no_materials, 1020, 1);
        assert!(conservative.is_empty());
    }

    #[test]
    fn minimum_light_life_does_not_extend_visual_primitives() {
        let mut catalog = FxLightPromotionCatalog::default();
        catalog.materials.insert(scene::canonical_shader_name("gfx/effects/testglow"),
            Some(MaterialLightAnalysis { static_emission: true, score: 6, texture_color: [1.0; 3],
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

fn fixed_light_profile(effect: &template::EffectTemplate, catalog: &FxLightPromotionCatalog) -> Option<(scene::DynamicLight,f32)> {
    let authored=effect.primitives.iter().find(|p|p.kind==PrimType::Light);
    let p=authored.or_else(|| {
        let profile=effect.auto_light?;
        if profile.spot.is_some() { return None; }
        effect.primitives.get(profile.driver_primitive)
    })?;
    if !matches!(p.kind,PrimType::Light|PrimType::Particle|PrimType::OrientedParticle) {return None;}
    let fixed=|range:template::Range|range.min.is_finite() && range.min==range.max;
    let zero=|range:template::Range|fixed(range) && range.min==0.0;
    if !p.velocity.iter().chain(p.acceleration.iter()).chain(p.origin1.iter()).all(|v|zero(*v))
        || !zero(p.gravity) || !zero(p.spawn_delay)
        || !fixed(p.spawn_count) || p.spawn_count.min!=1.0
        || !fixed(p.life) || !fixed(p.size_start) || p.size_start!=p.size_end
        || p.rgb_start!=p.rgb_end || !p.rgb_start.iter().all(|v|fixed(*v))
        || !fixed(p.alpha_start) || p.alpha_start!=p.alpha_end {return None;}
    let mut color=p.rgb_start.map(|v|v.min.max(0.0));
    let radius=if authored.is_some() {p.size_start.min} else {
        if p.media.len()!=1 { return None; }
        let analysis=catalog.material_analysis(&p.media[0])?;
        let profile=effect.auto_light?;
        if !analysis.static_emission || profile.score+analysis.score<FINAL_PROMOTION_SCORE {return None;}
        for i in 0..3 {color[i]=analysis.texture_color[i]*if analysis.rgb_vertex {color[i]} else {analysis.rgb_const[i]};}
        let alpha=if analysis.alpha_vertex {p.alpha_start.min} else {analysis.alpha_const};
        if matches!(analysis.blend,FxBlend::AddAlpha|FxBlend::Alpha) { for v in &mut color {*v *= alpha.clamp(0.0,1.0);} }
        (56.0+p.size_start.min.abs().max(1.0)*8.0).clamp(48.0,384.0)
    };
    if !radius.is_finite() || radius<=0.0 || color.iter().all(|v|*v<=0.0) {return None;}
    Some((scene::DynamicLight {position:[0.0;3],color,radius,intensity:1.0,shadow_power:radius*radius*0.35,
        falloff:scene::DynamicLightFalloff::Smooth,surface_lighting:true,emitter_normal:[0.0;3],
        emitter_two_sided:false,angle_attenuation:true,angle_scale:0.0,extra_distance:0.0,
        shader_promotion_mask:1<<4},p.life.min))
}

#[cfg(test)]
mod static_bake_tests {
    use super::*;
    #[test]
    fn baked_local_fx_accepts_only_fixed_continuous_untargeted_runners() {
        let effect=template::parse_effect("fixed",&super::super::gp2::parse(b"Light {\nlife 1000\nsize {\nstart 128\nend 128\n}\n}\n"));
        let mut catalog=FxLightPromotionCatalog::default();
        let profile=fixed_light_profile(&effect,&catalog).expect("fixed authored point light");
        catalog.fixed_effects.insert("fixed".into(),profile);
        let runner=scene::MapFxRunner {bake_static:true,source_entity_index:0,effect:"effects/fixed.efx".into(),origin:[10.0,20.0,30.0],angles:[0.0;3],delay_ms:200,random_ms:0,spawnflags:0};
        let lights=catalog.static_runner_lights(std::slice::from_ref(&runner));
        assert_eq!(lights.len(),1);assert_eq!(lights[0].position,scene::render_position(runner.origin));
        for changed in [scene::MapFxRunner {bake_static:false,..runner.clone()},scene::MapFxRunner {spawnflags:1,..runner.clone()},scene::MapFxRunner {spawnflags:2,..runner.clone()},scene::MapFxRunner {random_ms:20,..runner.clone()},scene::MapFxRunner {delay_ms:2000,..runner.clone()}] {
            assert!(catalog.static_runner_lights(&[changed]).is_empty());
        }
        let mut moving=effect.clone();moving.primitives[0].velocity[0]=template::Range::fixed(1.0);
        assert!(fixed_light_profile(&moving,&catalog).is_none());
        let mut flicker=effect;flicker.primitives[0].rgb_end[0]=template::Range::fixed(0.0);
        assert!(fixed_light_profile(&flicker,&catalog).is_none());
    }
}
