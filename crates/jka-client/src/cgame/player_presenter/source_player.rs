//! Source 1 / Garry's Mod player models.
//!
//! A Source character is rebound onto the native 72-bone JK2 rig at load time by
//! [`jka_assets::source1_rig`], so what reaches the rest of the presenter is an
//! ordinary [`GlmModel`] with GLA bone weights. Animation, torso/legs blending,
//! bolts, saber and weapon attachment, ragdoll, gore, shadows and ray tracing
//! therefore need no Source-specific code at all, and normal GLM players are
//! completely unaffected: nothing below runs unless
//! `models/players/<name>/source_model.json` exists.
use super::*;
use jka_assets::source1::{parse_source1_materials, parse_source1_skeleton};
use jka_assets::source1_geometry::{parse_source_vtx, parse_source_vvd};
use jka_assets::source1_rig::{
    attach_humanoid_bolts_between_rigs, build_source_rig, SourceRebindMode, SourceRigDiagnostics,
    SourceRigOptions,
};

// `PlayerModelAsset`, `PlayerSurfaceAsset`, `build_lod_gpu_meshes`, `Arc`,
// `HashMap`, `TextureData`, `DynamicModelAlphaMode` and `JiggleRemeshTuning`
// all arrive through `use super::*`.
use super::assets::loading::ModelSource;
use super::view::MAX_GLM_BYTES;
use crate::renderer::SourceMaterialParams;

pub(in crate::cgame::player_presenter) const MAX_DESCRIPTOR_BYTES: usize = 64 * 1024;
const MAX_VMT_BYTES: usize = 1024 * 1024;
const MAX_VTF_BYTES: usize = 64 * 1024 * 1024;
/// Gameplay animation source. Mesh binding uses the separate JK2 reference.
const HUMANOID_GLA: &str = "models/players/_humanoid/_humanoid.gla";
/// Bolts come from a stock humanoid's `*` tag surfaces. Kyle is OpenJK's own
/// registration fallback, so it is the model guaranteed to be present.
const DEFAULT_BOLT_DONOR: &str = "models/players/kyle/model.glm";

pub(in crate::cgame::player_presenter) fn source_descriptor_qpath(model_name: &str) -> String {
    format!("models/players/{model_name}/source_model.json")
}

/// `source_model.json`: the only authored file a Source character needs.
struct SourceDescriptor {
    /// Path to the `.mdl`; its `.vvd` and `.dx90.vtx` siblings are derived.
    model: String,
    /// `$bodygroup` choice per bodypart.
    bodygroups: Vec<usize>,
    /// `$texturegroup` row.
    skin_family: usize,
    /// Uniform Source-to-JKA scale; `None` fits it from the two skeletons.
    scale: Option<f32>,
    max_lods: usize,
    female: bool,
    jigglebones: bool,
    /// `"rebind"`: how Source geometry is re-placed onto its GLA bone. An authoring
    /// aid for comparing the constructions; the default is the correct one.
    rebind: SourceRebindMode,
    /// `"bolt_donor"`: stock `_humanoid` GLM whose `*` tag surfaces supply this
    /// character's bolts. A Source `.mdl` has no tag surfaces of its own.
    bolt_donor: String,
}

fn descriptor_number(value: Option<&serde_json::Value>, what: &str) -> Result<usize, String> {
    match value {
        None => Ok(0),
        Some(value) => value
            .as_u64()
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(|| format!("{what} must be a non-negative integer")),
    }
}

fn parse_descriptor(qpath: &str, bytes: &[u8]) -> Result<SourceDescriptor, String> {
    let metadata: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| format!("{qpath}: {error}"))?;
    if metadata.get("format").and_then(|v| v.as_str()) != Some("source1") {
        return Err(format!("{qpath}: not a source1 descriptor"));
    }
    if metadata.get("schema_version").and_then(|v| v.as_u64()) != Some(1) {
        return Err(format!("{qpath}: unsupported schema_version"));
    }
    let model = metadata
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{qpath}: missing model path"))?;
    // The descriptor names a file inside the package, so reject anything that
    // could escape the virtual filesystem's `models/` root.
    if !model.starts_with("models/")
        || !model.ends_with(".mdl")
        || model.contains("..")
        || model.contains('\\')
    {
        return Err(format!("{qpath}: unsafe model path {model:?}"));
    }
    let bodygroups = match metadata.get("bodygroups") {
        None => Vec::new(),
        Some(serde_json::Value::Array(entries)) => entries
            .iter()
            .map(|entry| descriptor_number(Some(entry), "bodygroups entry"))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("{qpath}: {error}"))?,
        Some(_) => return Err(format!("{qpath}: bodygroups must be an array")),
    };
    let scale = match metadata.get("scale") {
        None => None,
        Some(value) => {
            let scale = value
                .as_f64()
                .ok_or_else(|| format!("{qpath}: scale must be a number"))?
                as f32;
            if !scale.is_finite() || scale <= 0.0 {
                return Err(format!("{qpath}: scale {scale} is not positive"));
            }
            Some(scale)
        }
    };
    let max_lods = match metadata.get("max_lods") {
        None => jka_assets::source1_geometry::SOURCE_MAX_LODS,
        Some(value) => descriptor_number(Some(value), "max_lods")
            .map_err(|error| format!("{qpath}: {error}"))?
            .max(1),
    };
    Ok(SourceDescriptor {
        model: model.to_owned(),
        bodygroups,
        skin_family: descriptor_number(metadata.get("skin"), "skin")
            .map_err(|error| format!("{qpath}: {error}"))?,
        scale,
        max_lods,
        female: metadata
            .get("female")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        // Optional per-model A/B switch. Does not discard extra skin weights:
        // a disabled rig simply animates all its extra bones with their parent.
        jigglebones: metadata.get("jigglebones")
            .and_then(|v| v.as_bool()).unwrap_or(true),
        bolt_donor: match metadata.get("bolt_donor").and_then(|v| v.as_str()) {
            None => DEFAULT_BOLT_DONOR.to_owned(),
            Some(path) => {
                if !path.starts_with("models/") || !path.ends_with(".glm") || path.contains("..") {
                    return Err(format!("{qpath}: unsafe bolt_donor path {path:?}"));
                }
                path.to_owned()
            }
        },
        rebind: match metadata.get("rebind").and_then(|v| v.as_str()) {
            None => SourceRebindMode::default(),
            Some(name) => SourceRebindMode::from_descriptor(name).ok_or_else(|| {
                format!("{qpath}: unknown rebind mode {name:?} (direction, translation, bone_frame)")
            })?,
        },
    })
}

fn read_required(
    source: &mut dyn ModelSource,
    qpath: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    source
        .read_optional(qpath, max_bytes)?
        .ok_or_else(|| format!("missing {qpath}"))
}

/// Read and decode one VMT texture reference. A texture this decoder cannot read
/// must not delete the whole character, so every failure is logged and skipped:
/// the surface stays visible, which is obvious on screen, and the console names
/// the file.
fn resolve_source_texture(
    source: &mut dyn ModelSource,
    vmt_qpath: &str,
    path: &str,
    decode: fn(&str, &[u8]) -> Result<TextureData, String>,
) -> Result<Option<Arc<TextureData>>, String> {
    let vtf_qpath = format!("materials/{path}.vtf");
    let Some(vtf) = source.read_optional(&vtf_qpath, MAX_VTF_BYTES)? else {
        rverbose!(
            1,
            "SOURCE MATERIAL WARNING: {vmt_qpath} references missing {vtf_qpath}"
        );
        return Ok(None);
    };
    match decode(&vtf_qpath, &vtf) {
        Ok(decoded) => Ok(Some(Arc::new(decoded))),
        Err(error) => {
            println!("SOURCE MATERIAL WARNING: {error}");
            Ok(None)
        }
    }
}

/// Resolve one surface's VMT to a decoded `$basetexture` plus, when the material
/// declares them, its `$bumpmap` and phong block.
fn resolve_source_material(
    source: &mut dyn ModelSource,
    candidates: &[String],
) -> Result<SourceMaterial, String> {
    let mut tried = Vec::new();
    for vmt_qpath in candidates {
        let Some(vmt) = source.read_optional(vmt_qpath, MAX_VMT_BYTES)? else {
            tried.push(vmt_qpath.clone());
            continue;
        };
        let parameters = super::source_vtf::parse_vmt(&vmt);
        let mut material = SourceMaterial {
            alpha_mode: if parameters.translucent {
                DynamicModelAlphaMode::Blend
            } else if parameters.alpha_test {
                DynamicModelAlphaMode::Mask
            } else {
                DynamicModelAlphaMode::Opaque
            },
            ..SourceMaterial::default()
        };
        if let Some(base) = parameters.base_texture.as_deref() {
            material.texture = resolve_source_texture(
                source,
                vmt_qpath,
                base,
                super::source_vtf::decode_color_vtf,
            )?;
        }
        let normal_map = match parameters.bump_map.as_deref() {
            Some(bump) => resolve_source_texture(
                source,
                vmt_qpath,
                bump,
                super::source_vtf::decode_normal_vtf,
            )?,
            None => None,
        };
        // A VMT with neither a bump map nor phong is lit exactly like a JKA
        // surface, so it must not be pushed onto the costlier pipeline variant.
        if normal_map.is_some() || parameters.phong.is_some() || parameters.half_lambert {
            let phong = parameters.phong.unwrap_or_default();
            material.source = Some(SourceMaterialParams {
                normal_map,
                phong_boost: if parameters.phong.is_some() {
                    phong.boost
                } else {
                    0.0
                },
                phong_exponent: phong.exponent,
                phong_fresnel: phong.fresnel_ranges,
                rimlight_boost: if phong.rimlight {
                    phong.rimlight_boost
                } else {
                    0.0
                },
                rimlight_exponent: phong.rimlight_exponent,
                half_lambert: parameters.half_lambert,
            });
        }
        return Ok(material);
    }
    // A missing VMT must not delete the geometry: the surface stays visible and
    // untextured so the model is still inspectable, and the reason is logged.
    if !tried.is_empty() {
        rverbose!(
            1,
            "SOURCE MATERIAL WARNING: none of {} exist",
            tried.join(", ")
        );
    }
    Ok(SourceMaterial::default())
}

struct SourceMaterial {
    texture: Option<Arc<TextureData>>,
    alpha_mode: DynamicModelAlphaMode,
    /// `$bumpmap` and the phong block, when the VMT declares either.
    source: Option<SourceMaterialParams>,
}

impl Default for SourceMaterial {
    fn default() -> Self {
        Self {
            texture: None,
            alpha_mode: DynamicModelAlphaMode::Opaque,
            source: None,
        }
    }
}

/// Build a `PlayerModelAsset` from a Source 1 character. The returned asset is
/// indistinguishable from a GLM one to every consumer.
pub(in crate::cgame::player_presenter) fn build_source_player_model(
    source: &mut dyn ModelSource,
    model_name: &str,
    key: String,
    descriptor_qpath: &str,
    descriptor_bytes: &[u8],
    jiggle_remesh: JiggleRemeshTuning,
) -> Result<PlayerModelAsset, String> {
    let descriptor = parse_descriptor(descriptor_qpath, descriptor_bytes)?;
    let base = descriptor
        .model
        .strip_suffix(".mdl")
        .ok_or_else(|| format!("{descriptor_qpath}: model path must end in .mdl"))?;

    let mdl = read_required(source, &descriptor.model, MAX_GLM_BYTES)?;
    let vvd = parse_source_vvd(&read_required(source, &format!("{base}.vvd"), MAX_GLM_BYTES)?)
        .map_err(|error| format!("{base}.vvd: {error}"))?;
    let vtx = parse_source_vtx(&read_required(
        source,
        &format!("{base}.dx90.vtx"),
        MAX_GLM_BYTES,
    )?)
    .map_err(|error| format!("{base}.dx90.vtx: {error}"))?;
    let skeleton =
        parse_source1_skeleton(&mdl).map_err(|error| format!("{}: {error}", descriptor.model))?;
    let materials =
        parse_source1_materials(&mdl).map_err(|error| format!("{}: {error}", descriptor.model))?;
    let animation_gla = source.load_gla(HUMANOID_GLA)?;
    let gla = jka_assets::skeleton::humanoid_reference(jka_assets::skeleton::HumanoidRig::Jk2_72);
    let retargeter = Some(jka_assets::skeleton::cached_retargeter(&animation_gla.skeleton, &gla.skeleton)?);

    let mut rig = build_source_rig(
        &descriptor.model,
        &mdl,
        &vvd,
        &vtx,
        &skeleton,
        &materials,
        &gla,
        &SourceRigOptions {
            bodygroups: descriptor.bodygroups.clone(),
            skin_family: descriptor.skin_family,
            max_lods: descriptor.max_lods,
            scale: descriptor.scale,
            rebind: descriptor.rebind,
        },
    )?;
    let mut body_shape_available = false;
    let mut body_shape_binding = None;
    let shape_qpath = format!("models/players/{model_name}/model.bodyshape");
    if let Some(bytes) = source.read_optional(&shape_qpath, 64 * 1024 * 1024)? {
        match jka_assets::body_shape::BodyShapeGrid::parse(
            &bytes, &rig.glm, jka_assets::body_shape::model_fingerprint(&mdl),
        ) {
            Ok(grid) => {
                body_shape_available = true;
                if let Some(pivots) = grid.apply(&mut rig.glm, &gla, jiggle_remesh.body_shape, rig.native_pivots.as_deref())? {
                    body_shape_binding = Some(Arc::new(jka_assets::body_shape::BodyShapeBinding::new(
                        &gla, rig.native_pivots.as_deref(), &pivots,
                    )?));
                    // Match the procedural hair pivots to the shaped native
                    // cranium/parent pivot, just as the ordinary GLA bone
                    // matrices are rebound by BodyShapeBinding.
                    if let Some(original) = rig.native_pivots.as_deref() {
                        for jiggle_bone in &mut rig.source_jiggles {
                            let parent = jiggle_bone.parent_gla;
                            for k in 0..3 {
                                jiggle_bone.rest_frame[k][3] += pivots[parent][k] - original[parent][k];
                            }
                        }
                    }
                    rig.native_pivots = Some(pivots);
                }
            }
            Err(error) => rverbose!(1, "SOURCE BODY SHAPE WARNING: {model_name}: {error}"),
        }
    }
    // A Source `.mdl` has no Ghoul2 tag surfaces, so without these the character
    // has no bolts at all and the saber, weapon flash and jetpack silently vanish.
    let bolts = match source.read_optional(&descriptor.bolt_donor, MAX_GLM_BYTES)? {
        Some(bytes) => {
            let donor = jka_assets::ghoul2::parse_glm_native(&bytes)
                .map_err(|error| format!("{}: {error}", descriptor.bolt_donor))?;
            let donor_gla = match donor.num_bones {
                53 => jka_assets::skeleton::humanoid_reference(jka_assets::skeleton::HumanoidRig::Jka53),
                72 => Arc::clone(&gla),
                _ => return Err("Source bolt donor must use a 53- or 72-bone humanoid rig".into()),
            };
            attach_humanoid_bolts_between_rigs(&mut rig, &donor, &donor_gla, &gla)
                .map_err(|error| format!("{}: {error}", descriptor.bolt_donor))?
        }
        None => {
            println!(
                "SOURCE PLAYER WARNING: {model_name} bolt donor {} is missing; \
                 saber, weapon and bolt-driven effects will not attach",
                descriptor.bolt_donor
            );
            0
        }
    };
    let source_native_pivots = rig.native_pivots.map(Arc::new);
    let source_jiggles = (!rig.source_jiggles.is_empty()).then(|| Arc::new(rig.source_jiggles));
    let glm = Arc::new(rig.glm);
    // Source geometry is already rebound and weighted to _humanoid. The
    // stock GLM auto detector requires Raven helper surfaces, which Source
    // models don't have; use the anatomical Source detector instead.
    let jiggle = match JiggleProfile::auto_source_humanoid(
        &glm,
        &gla,
        source_native_pivots.as_deref().map(Vec::as_slice),
    ) {
        Ok(Some(profile)) => {
            devprintln!(
                2,
                "SOURCE PLAYER JIGGLE: {model_name} auto regions={}",
                profile.region_count(),
            );
            Some(Arc::new(profile))
        }
        Ok(None) => None,
        Err(error) => {
            rverbose!(1, "SOURCE PLAYER JIGGLE AUTO WARNING: {model_name}: {error}");
            None
        }
    };
    let joined_breast_bakes = build_joined_breast_bakes(&glm, jiggle.as_deref(), jiggle_remesh)?;

    let mut surfaces = Vec::with_capacity(rig.surfaces.len());
    for (surface_index, surface) in rig.surfaces.iter().enumerate() {
        let material = resolve_source_material(source, &surface.vmt_candidates)?;
        surfaces.push(PlayerSurfaceAsset {
            surface_index,
            default_visible: true,
            surface_name: surface.name.clone(),
            texture: material.texture,
            source_material: material.source,
            alpha_mode: material.alpha_mode,
            authored_glow: false,
            shader_time_offset: 0.0,
            shader_clamp_time: None,
            fallback_gray: false,
            entity_tint: false,
            overlay: None,
            stages: Vec::new(),
            gpu_meshes: build_lod_gpu_meshes(
                &descriptor.model,
                &glm,
                surface_index,
                false,
                jiggle.as_deref(),
                jiggle_remesh,
                &joined_breast_bakes,
            )?,
        });
    }

    log_source_diagnostics(model_name, &descriptor, &rig.diagnostics, bolts);
    if rig.diagnostics.source_jiggle_bones > 0 {
        println!("SOURCE JIGGLEBONES: {model_name}: {} procedural bones, {} weighted influences, simulation={} (source_model.json jigglebones)",
            rig.diagnostics.source_jiggle_bones,
            rig.diagnostics.source_jiggle_weighted_vertices,
            descriptor.jigglebones);
    }

    Ok(PlayerModelAsset {
        body_shape_available,
        body_shape_binding,
        key,
        glm,
        gla,
        animation_gla,
        retargeter,
        source_native_pivots,
        source_jiggles,
        source_jiggles_enabled: descriptor.jigglebones,
        emote_contacts: Default::default(),
        // A Source player is driven by `players/_humanoid/`, so it takes the
        // ordinary multiplayer animation and BG_G2PlayerAngles path.
        humanoid: true,
        animations: None,
        anim_events: None,
        surfaces,
        jiggle,
        female: descriptor.female,
    })
}

fn log_source_diagnostics(
    model_name: &str,
    descriptor: &SourceDescriptor,
    diagnostics: &SourceRigDiagnostics,
    bolts: usize,
) {
    let lods = diagnostics
        .lods
        .iter()
        .map(|lod| format!("lod{}={}t", lod.lod, lod.triangles))
        .collect::<Vec<_>>()
        .join(" ");
    println!(
        "SOURCE PLAYER REGISTERED: {model_name} model={} bones={} mapped={} inherited={} \
         rebind={:?} dirFixed={} scale={:.4}{} fitRMS={:.2} yaw={:.1} reshaped={:.2} \
         surfaces={} bolts={bolts} {lods}",
        descriptor.model,
        diagnostics.source_bones,
        diagnostics.mapped_bones,
        diagnostics.inherited_bones.len(),
        diagnostics.rebind,
        diagnostics.direction_corrected,
        diagnostics.scale,
        if diagnostics.scale_was_fitted { "" } else { " (authored)" },
        diagnostics.fit_rms,
        diagnostics.fit_yaw_degrees,
        diagnostics.max_rebind_displacement,
        diagnostics.lods.first().map_or(0, |lod| lod.surfaces),
    );
    // A large alignment residual or bind-pose disagreement is the first thing to
    // look at when a Source character animates oddly, so surface it by default
    // rather than burying it behind a verbosity level.
    if diagnostics.fit_rms > 6.0 {
        println!(
            "SOURCE PLAYER WARNING: {model_name} skeleton differs from _humanoid by \
             {:.2} units RMS{}; limbs will be noticeably repositioned by the rebind",
            diagnostics.fit_rms,
            diagnostics
                .fit_worst
                .as_ref()
                .map(|(source, target, error)| format!(
                    " (worst {source} vs {target}, {error:.2})"
                ))
                .unwrap_or_default(),
        );
    }
    if diagnostics.max_inverse_bind_error > 0.05 {
        println!(
            "SOURCE PLAYER WARNING: {model_name} authored poseToBone disagrees with the \
             derived inverse bind by {:.4}; the MDL bind pose may be unusual",
            diagnostics.max_inverse_bind_error
        );
    }
    // Source stores at most three influences per vertex and merging onto GLA bones
    // can only reduce that, so Ghoul2's four-weight limit is unreachable here.
    if diagnostics.weight_truncations > 0 {
        println!(
            "SOURCE PLAYER WARNING: {model_name} truncated {} vertices to four weights, \
             which should be impossible for a 3-influence Source vertex",
            diagnostics.weight_truncations
        );
    }
    if diagnostics.max_rebind_displacement > 5.0 {
        println!(
            "SOURCE PLAYER WARNING: {model_name} rebind moved geometry up to {:.2} units; \
             expect visible reshaping (mode {:?})",
            diagnostics.max_rebind_displacement, diagnostics.rebind
        );
    }
    for (source, target) in &diagnostics.inherited_bones {
        rverbose!(2, "SOURCE PLAYER BONE: {model_name} {source} follows {target}");
    }
}
